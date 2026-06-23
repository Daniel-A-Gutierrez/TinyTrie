use std::borrow::Borrow;
use std::cmp::Ordering;
use std::num::{NonZero, ZeroablePrimitive};
use std::simd::cmp::SimdPartialOrd;
use std::simd::Simd;

use crate::tiny_array::TinyArray;

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Index type for arena-based node pointers.
///
/// Reuses the same pattern as the other tries in this project:
/// `u8`, `u16`, `u32`, `u64` with `NonZero` packing.
pub trait TrieIndex:
    Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug + 'static + ZeroablePrimitive
{
    fn as_usize(self) -> usize;
    fn max_value() -> usize;
    fn from_usize(n: usize) -> Self;
}

/// Fixed-size keys that can be compared with SIMD broadcast.
pub trait FixedLenKey: Copy + Eq + Ord + Sized {
    /// Find the first index `i` in `haystack` where `haystack[i] >= needle`.
    /// Returns `haystack.len()` if all elements are less than `needle`.
    fn find_position(needle: &Self, haystack: &[Self]) -> usize ;
    fn find_upper_bound(needle: &Self, haystack: &[Self]) -> usize;
}

/// Variable-length key: a sequence of `K: FixedLenKey` chunks.
///
/// `VarLenKey<u8>` = byte string, `VarLenKey<u32>` = u32 word sequence, etc.
pub trait VarLenKey<K: FixedLenKey>: Eq + Ord + Sized {
    fn as_chunks(&self) -> &[K];
    fn chunk_len(&self) -> usize {
        self.as_chunks().len()
    }
}

// ---------------------------------------------------------------------------
// FixedLenKey impls — u8, u16, u32, u64 with SIMD find_position
// ---------------------------------------------------------------------------

/// SIMD `find_position` for types that use `from_slice`.
/// $ty: the primitive type, $lanes: SIMD vector width.
macro_rules! impl_fixed_len_key_simd {
    ($ty:ty, $lanes:expr) => {
        impl FixedLenKey for $ty {
            #[inline]
            fn find_position(needle: &Self, haystack: &[Self]) -> usize {
                let len = haystack.len();
                if len == 0 {
                    return 0;
                }
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
                    if haystack[i] >= *needle {
                        return i;
                    }
                    i += 1;
                }
                len
            }
            #[inline]
            fn find_upper_bound(needle: &Self, haystack: &[Self]) -> usize {
                let len = haystack.len();
                if len == 0 {
                    return 0;
                }
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
                    if haystack[i] > *needle {
                        return i;
                    }
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

// ---------------------------------------------------------------------------
// VarLenKey impls
// ---------------------------------------------------------------------------

impl VarLenKey<u8> for Vec<u8> {
    #[inline]
    fn as_chunks(&self) -> &[u8] {
        self.as_slice()
    }
}

impl VarLenKey<u8> for &[u8] {
    #[inline]
    fn as_chunks(&self) -> &[u8] {
        self
    }
}

impl VarLenKey<u8> for Box<[u8]> {
    #[inline]
    fn as_chunks(&self) -> &[u8] {
        self
    }
}

// ---------------------------------------------------------------------------
// StoredKey — sealed internal search trait for the two canonical key forms
// ---------------------------------------------------------------------------
//
// The B+ tree stores keys in one of two canonical representations:
//   * a fixed-size `K: FixedLenKey`          — searched via SIMD broadcast
//   * a variable-length `Box<[K]>`           — searched via binary search
//
// This trait is the single point of variation between the two: `find_position`
// and `find_upper_bound` dispatch to the form's own search. It is sealed because it
// is the custodian of the SIMD-haystack invariant — the stored key array must
// be a contiguous `&[Self]` for `Simd::from_slice` to be sound-by-convention.
// Only the two forms above may implement it. Consumers never name this trait;
// they reach the tree through the stdlib `Borrow`/`Into` conversion seam.
//
// The lookup needle type is an associated type, not a method generic: `K` for
// the fixed form, `[K]` for the variable form. Each impl performs its own
// concrete comparison, so no `PartialOrd` bound leaks into the trait surface.

mod private {
    /// Marker sealing [`super::StoredKey`] to the two canonical key forms.
    pub trait Sealed {}
}

pub trait StoredKey: Ord + Clone + private::Sealed
where
    Self: Borrow<Self::Needle>,
{
    /// Borrowed lookup needle: `K` for fixed, `[K]` for variable.
    type Needle: ?Sized;

    /// First index `i` where `haystack[i] >= needle` (lower bound).
    /// Returns `haystack.len()` if all elements are less than `needle`.
    fn find_position(needle: &Self::Needle, haystack: &[Self]) -> usize;

    /// First index `i` where `haystack[i] > needle` (upper bound).
    /// Returns `haystack.len()` if no element is greater than `needle`.
    fn find_upper_bound(needle: &Self::Needle, haystack: &[Self]) -> usize;

    /// Is `stored` equal to `needle`? Form-specific: `PartialEq` between a
    /// stored key and its needle is not uniformly in scope across both forms
    /// (e.g. `Box<[K]>: PartialEq<[K]>` is not a stdlib impl), so the trait
    /// owns the comparison. Used to confirm an exact hit after `find_position`.
    fn eq_key(stored: &Self, needle: &Self::Needle) -> bool;
}

// --- Fixed form: K: FixedLenKey — SIMD lower + upper bounds -----------------

impl<K: FixedLenKey> private::Sealed for K {}

impl<K: FixedLenKey> StoredKey for K {
    type Needle = K;

    #[inline]
    fn find_position(needle: &Self::Needle, haystack: &[Self]) -> usize {
        // Delegate to the SIMD-accelerated `FixedLenKey::find_position`.
        K::find_position(needle, haystack)
    }

    #[inline]
    fn find_upper_bound(needle: &Self::Needle, haystack: &[Self]) -> usize {
        K::find_upper_bound(needle, haystack)
    }

    #[inline]
    fn eq_key(stored: &Self, needle: &Self::Needle) -> bool {
        stored == needle
    }
}

// --- Variable form: Box<[K]> — binary search both directions ---------------

impl<K: FixedLenKey> private::Sealed for Box<[K]> {}

impl<K: FixedLenKey> StoredKey for Box<[K]> {
    type Needle = [K];

    #[inline]
    fn find_position(needle: &Self::Needle, haystack: &[Self]) -> usize {
        let mut lo = 0usize;
        let mut hi = haystack.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // SAFETY: items 0..len are guaranteed initialized in a live node.
            let node_key = unsafe { haystack.get_unchecked(mid) };
            match cmp_slice_scalar(node_key.as_ref(), needle) {
                Ordering::Less => lo = mid + 1,
                Ordering::Equal => return mid,
                Ordering::Greater => hi = mid,
            }
        }
        lo
    }

    #[inline]
    fn find_upper_bound(needle: &Self::Needle, haystack: &[Self]) -> usize {
        let mut lo = 0usize;
        let mut hi = haystack.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // SAFETY: items 0..len are guaranteed initialized in a live node.
            let node_key = unsafe { haystack.get_unchecked(mid) };
            if cmp_slice_scalar(node_key.as_ref(), needle) == Ordering::Greater {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }

    #[inline]
    fn eq_key(stored: &Self, needle: &Self::Needle) -> bool {
        eq_slice_scalar(stored.as_ref(), needle)
    }
}

// Scalar comparison helpers for the varlen (`Box<[K]>`) binary search. These
// are plain element-wise `Ord`/`PartialEq` loops — no SIMD. Benchmarks showed
// the SIMD `cmp_slice`/`eq_slice` variant made no measurable difference here, so
// it was removed.
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
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Node types — unified over SK: StoredKey
// ---------------------------------------------------------------------------

/// Internal (separator) node: `keys` plus `ptrs` to children.
struct KeyNode<SK, PTR, const N: usize, const NP1: usize>
where
    SK: StoredKey,
    PTR: TrieIndex,
    [(); N]:
    ,
    [(); NP1]:
{
    keys: TinyArray<SK, N>,
    ptrs: [Option<NonZero<PTR>>; NP1],
}

/// Leaf node. Carries the leaf linked list (`prev`/`next`) for O(1) cursor
/// navigation in both the fixed and variable instantiations.
///
/// `prev`/`next` use the same `Option<NonZero<PTR>>` encoding as `KeyNode`'s
/// child ptrs: stored as 1-based `NonZero`, decoded to a 0-based arena index.
struct LeafNode<SK, V, PTR, const N: usize>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
{
    keys: TinyArray<SK, N>,
    values: TinyArray<V, N>,
    prev: Option<NonZero<PTR>>,
    next: Option<NonZero<PTR>>,
}

// ---------------------------------------------------------------------------
// CTree — generic B+ tree over SK: StoredKey
// ---------------------------------------------------------------------------

/// B+ tree. The key representation is chosen by `SK: StoredKey`:
///   * `CTree<K, V, ...>`        — fixed-size keys, SIMD search (`FixedCTree`)
///   * `CTree<Box<[K]>, V, ...>` — variable-length keys, binary search (`VarCTree`)
///
/// All tree operations are implemented once, generically; `find_position` and
/// `find_upper_bound` dispatch to `SK`. Consumers reach the tree through the
/// stdlib `Borrow`/`Into` conversion seam — they never implement search.
pub struct CTree<SK, V, PTR, const N: usize, const NP1: usize>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    inodes: Vec<KeyNode<SK, PTR, N, NP1>>,
    leaves: Vec<LeafNode<SK, V, PTR, N>>,
    len: usize,
    /// Count of LIVE leaves (excludes gap sentinels). `leaves.len()` is the
    /// arena slot count (live + gaps); the two diverge whenever the gap arena
    /// has free slots. See `spread` / `claim_slot`.
    n_leaves: usize,
    /// Number of inode levels. 0 = root is a leaf, 1 = root inode above leaves, etc.
    height: usize,
    /// Index of the root inode in `self.inodes`. Only valid when height >= 1.
    root_inode: usize,
}

pub struct Cursor<'a, SK, V, PTR, const N: usize, const NP1: usize>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    tree: &'a CTree<SK, V, PTR, N, NP1>,
    leaf_idx: usize,
    position: usize,
}

pub struct CursorMut<'a, SK, V, PTR, const N: usize, const NP1: usize>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    tree: &'a mut CTree<SK, V, PTR, N, NP1>,
    leaf_idx: usize,
    position: usize,
}

// ---------------------------------------------------------------------------
// KeyNode impl
// ---------------------------------------------------------------------------

impl<SK, PTR, const N: usize, const NP1: usize> KeyNode<SK, PTR, N, NP1>
where
    SK: StoredKey,
    PTR: TrieIndex,
    [(); N]:
    ,
    [(); NP1]:
{
    // NP1 must be exactly N + 1 (one pointer per key, plus the rightmost child).
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

    /// Get key at index `i`. Bounds-checks against `len`.
    #[inline]
    fn get(&self, i: usize) -> &SK {
        self.keys.get(i)
    }

    /// Get key at index `i` without bounds check.
    #[inline]
    unsafe fn get_unchecked(&self, i: usize) -> &SK {
        unsafe {
            self.keys.get_unchecked(i)
        }
    }

    /// Get child pointer at index `i` as a usize index. Returns `None` for empty slots.
    /// Internally stored as 1-based NonZero; decoded to 0-based index.
    #[inline]
    fn get_ptr(&self, i: usize) -> Option<usize> {
        debug_assert!(i <= self.keys.len());
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    /// Get child pointer at index `i` without bounds check.
    #[inline]
    unsafe fn get_ptr_unchecked(&self, i: usize) -> Option<usize> {
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    /// Set child pointer at index `i` to the given 0-based arena index.
    /// Encoded as 1-based NonZero internally.
    #[inline]
    fn set_ptr(&mut self, i: usize, idx: usize) {
        self.ptrs[i] = NonZero::new(PTR::from_usize(idx + 1));
    }

    /// Clear child pointer at index `i`.
    #[inline]
    fn clear_ptr(&mut self, i: usize) {
        self.ptrs[i] = None;
    }

    /// First index where `keys[i] >= needle` (lower bound).
    #[inline]
    fn find_position(&self, needle: &SK::Needle) -> usize {
        SK::find_position(needle, self.keys.as_slice())
    }

    /// Find the child pointer index for `needle` in a B+ tree internal node.
    /// Returns the index `i` such that `ptrs[i]` points to the subtree for
    /// `needle`. Upper-bound semantics: finds the first separator `> needle`.
    #[inline]
    fn find_child(&self, needle: &SK::Needle) -> usize {
        SK::find_upper_bound(needle, self.keys.as_slice())
    }

    /// Child pointers of the two siblings adjacent to the child at `child_pos`.
    /// `(left sibling ptr, right sibling ptr)`; each is `None` when out of range.
    #[inline]
    fn adjacent_sibling_ptrs(&self, child_pos: usize) -> (Option<usize>, Option<usize>) {
        let left = if child_pos > 0 { self.get_ptr(child_pos - 1) } else { None };
        let right = if child_pos < self.keys.len() { self.get_ptr(child_pos + 1) } else { None };
        (left, right)
    }

    /// Would inserting one more key overflow this node?
    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    /// Would removing one more key drop below the minimum fill factor?
    #[inline]
    fn would_merge(&self) -> bool {
        self.keys.len() == N / 2
    }

    /// Insert `k` at the known sorted position `pos`, shifting the ptrs above
    /// it right by one. Caller guarantees `pos` is the correct insertion index
    /// and the node has room (`!would_split()`).
    fn insert_key_at(&mut self, pos: usize, k: SK) {
        debug_assert!(!self.would_split());
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, k);
    }

    /// Insert `k` into this internal node in sorted order.
    /// Also shifts ptrs. Caller guarantees `!would_split()`.
    /// Returns the position where the key was inserted.
    fn insert_leaf(&mut self, k: SK) -> usize {
        let pos = self.find_position(k.borrow());
        self.insert_key_at(pos, k);
        pos
    }

    /// Remove key at `pos` and its right child pointer.
    /// Returns the removed key.
    fn remove(&mut self, pos: usize) -> SK {
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

impl<SK, V, PTR, const N: usize> LeafNode<SK, V, PTR, N>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            values: TinyArray::new(),
            prev: None,
            next: None,
        }
    }

    /// Previous-leaf index, decoded to 0-based. `None` if this is the first leaf.
    /// Internally stored as 1-based NonZero, mirroring `KeyNode`'s child ptrs.
    #[inline]
    fn get_prev(&self) -> Option<usize> {
        self.prev.map(|nz| nz.get().as_usize() - 1)
    }

    /// Next-leaf index, decoded to 0-based. `None` if this is the last leaf.
    #[inline]
    fn get_next(&self) -> Option<usize> {
        self.next.map(|nz| nz.get().as_usize() - 1)
    }

    /// Set previous-leaf link to the given 0-based arena index. Encoded 1-based.
    #[inline]
    fn set_prev(&mut self, idx: usize) {
        self.prev = NonZero::new(PTR::from_usize(idx + 1));
    }

    /// Set next-leaf link to the given 0-based arena index. Encoded 1-based.
    #[inline]
    fn set_next(&mut self, idx: usize) {
        self.next = NonZero::new(PTR::from_usize(idx + 1));
    }

    /// Clear the previous-leaf link.
    #[inline]
    fn clear_prev(&mut self) {
        self.prev = None;
    }

    /// Clear the next-leaf link.
    #[inline]
    fn clear_next(&mut self) {
        self.next = None;
    }

    #[inline]
    fn find_position(&self, needle: &SK::Needle) -> usize {
        SK::find_position(needle, self.keys.as_slice())
    }

    /// Would inserting one more key overflow this leaf? Mirrors KeyNode::would_split.
    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    /// Insert key-value at position `pos`. Caller must ensure pos is correct
    /// (from `find_position`) and node is not full.
    fn insert(&mut self, pos: usize, k: SK, v: V) {
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
    }

    fn remove(&mut self, pos: usize) -> (SK, V) {
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
// two_mut — two simultaneous mutable borrows of distinct indices in a slice
// ---------------------------------------------------------------------------

/// Borrow two distinct indices of a slice mutably at once.
///
/// The arena trees store nodes in `Vec<KeyNode>`/`Vec<LeafNode>` and rebalance
/// needs to mutate two sibling nodes simultaneously (e.g. drain from one into
/// the other). `split_at_mut` gives one clean cut; this helper picks the cut so
/// the lower index lands in the left sub-slice and the higher in the right,
/// returning `(a_ref, b_ref)` regardless of which index is larger.
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

impl<SK, V, PTR, const N: usize, const NP1: usize> CTree<SK, V, PTR, N, NP1>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");
    // NP1 must be exactly N + 1 (one pointer per key, plus the rightmost child).
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    /// Target key count for the full node after rebalancing with a sibling
    /// holding `s` keys: the floor of the midpoint, so the full node keeps
    /// the smaller half and ends <= N-1.
    #[inline]
    fn rebalance_target(s: usize) -> usize {
        (N + s) / 2
    }

    pub fn new() -> Self {
        // Force evaluation of the static asserts (a const is only checked when
        // referenced). Catches `NP1 != N + 1` at the call site.
        let () = Self::ASSERT_NP1;
        let () = Self::ASSERT_N_FITS;
        // Start with one empty leaf node (the root). height = 0.
        let root = LeafNode::<SK, V, PTR, N>::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
            len: 0,
            n_leaves: 1,
            height: 0,
            root_inode: 0, // unused when height == 0
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Linearize the leaf arena and reclaim spare capacity in the node arenas.
    ///
    /// During a build, `split_leaf` places new leaves in the gap slot adjacent
    /// to their parent (via `spread`), so the natural layout is a gap-arena —
    /// live leaves at even slots, gaps at odd — and forward iteration is a
    /// clean stride-2 sweep with no front↔end bounce. `compact` rewrites the
    /// arena to drop the gaps: live leaves packed contiguously in linked-list
    /// (sorted) order, so iteration becomes stride-1, then shrinks `leaves`
    /// and `inodes` to exact length — the tree's steady-state footprint rather
    /// than its transient growth capacity. Useful before measuring memory, or
    /// to tighten a tree built once and queried often. After `compact` there are
    /// no gaps, so the next split's `claim_slot` finds no free slot and
    /// triggers a `spread` (the intended realloc moment).
    pub fn compact(&mut self) {
        // Pack live leaves contiguously (drop all gap sentinels), remap the
        // bottom-level inode ptrs and rewire prev/next, then shrink the node
        // arenas to exact length — the tree's steady-state footprint rather
        // than its transient growth capacity. After `compact` there are no
        // gaps, so the next split's `claim_slot` finds no free slot and
        // triggers a `spread` (which is the intended realloc moment).
        self.relocate(false);
        self.inodes.shrink_to_fit();
    }

    /// Physically reorder the leaf arena into linked-list (sorted) order so the
    /// leaf linked list is a contiguous scan: after `optimize`, `leaves[i].next`
    /// points to `leaves[i+1]` and `leaves[i].prev` to `leaves[i-1]`.
    ///
    /// This is now a thin shim over `compact`'s linearize pass: it produces the
    /// same contiguous, gap-free layout the old in-place `optimize` did, so
    /// existing callers (the bench's `CTreeOptBench`, the `test_optimize_*`
    /// tests) keep their semantics. The split-placement + `spread` machinery in
    /// `insert` keeps the natural layout bounce-free between `optimize` calls,
    /// so an explicit `optimize` is no longer required for fast iteration — but
    /// it still squeezes out the last stride-2 gap-arena layout into stride-1.
    pub fn optimize(&mut self) {
        self.relocate(false);
    }

    /// Rebuild the leaf arena, walking the live-leaf linked list in sorted
    /// order and placing each live leaf at its new slot. When `gapful` is true
    /// (`spread`), live leaves occupy the EVEN slots `[0, 2, 4, …]` and every
    /// ODD slot is a free gap sentinel — so a split can drop its new right-half
    /// leaf into the reserved gap at `parent_idx + 1` in O(1) with no shift and
    /// no front↔end bounce. When `gapful` is false (`compact`/`optimize`), live
    /// leaves are packed contiguously with no gaps (stride-1 iteration).
    ///
    /// This is the realloc moment: a fresh buffer is built at the chosen
    /// capacity (exactly `2 * live` or `live`, so the next growth triggers
    /// another explicit `spread`, never a silent `Vec` auto-grow), the live
    /// leaves are moved out of the old arena (leaving valid empty sentinels so
    /// it drops cleanly), bottom-level inode child pointers are remapped to the
    /// new positions, and `prev`/`next` are rewired. `n_leaves` is recomputed
    /// from the walk (self-correcting). Returns the old→new position map
    /// (`usize::MAX` for gap/unused old slots) so a caller mid-split can
    /// re-derive the indices it holds across the relocation.
    fn relocate(&mut self, gapful: bool) -> Vec<usize> {
        let old_len = self.leaves.len();
        // Walk the live-leaf linked list in sorted order. Gaps are never linked
        // or pointed-to, so this visits exactly the live leaves.
        let mut order: Vec<usize> = Vec::with_capacity(self.n_leaves);
        let mut idx = self.first_leaf();
        order.push(idx);
        while let Some(nx) = self.leaves[idx].get_next() {
            order.push(nx);
            idx = nx;
        }
        let live = order.len();
        debug_assert_eq!(
            live, self.n_leaves,
            "relocate: linked-list length {live} != n_leaves {}",
            self.n_leaves
        );

        let slot_of = |rank: usize| if gapful { 2 * rank } else { rank };
        let new_slots = slot_of(live);

        // old → new position map (usize::MAX for gap/unused old slots).
        let mut new_pos = vec![usize::MAX; old_len];
        for rank in 0..live {
            new_pos[order[rank]] = slot_of(rank);
        }

        // Take ownership of the old arena so leaves can be moved out cleanly;
        // each moved leaf is replaced with a valid empty sentinel so `old`
        // drops without double-free. `self.leaves` is left empty until the
        // fresh buffer is installed at the end.
        let mut old = std::mem::take(&mut self.leaves);
        let mut buf: Vec<LeafNode<SK, V, PTR, N>> = Vec::with_capacity(new_slots);
        for i in 0..new_slots {
            let is_live_slot = !gapful || i % 2 == 0;
            if is_live_slot {
                let rank = if gapful { i / 2 } else { i };
                let old_idx = order[rank];
                let leaf = std::mem::replace(&mut old[old_idx], LeafNode::new());
                buf.push(leaf);
            } else {
                // Gap sentinel: an empty `LeafNode` inside the arena.
                // `keys.len() == 0` is the unambiguous "this slot is free"
                // marker — a live leaf always holds ≥1 key.
                buf.push(LeafNode::new());
            }
        }
        drop(old);

        // Rewire prev/next to the new layout.
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

        // Remap the bottom-level inode child pointers (the ptrs that index into
        // `leaves`). Descend `height - 1` times from the root, expanding every
        // child, to collect exactly the inodes whose children are leaves.
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

    /// Spread the leaf arena: relocate live leaves to even slots with gap
    /// sentinels in every odd slot. Called by `split_leaf` when the arena is
    /// full (`n_leaves == leaves.len()`, no free gap) to recreate gaps before a
    /// new leaf is placed. Returns the old→new position map so the caller can
    /// re-derive the leaf index it holds across the relocation.
    fn spread(&mut self) -> Vec<usize> {
        self.relocate(true)
    }

    /// Find a free arena slot for a split's new right-half leaf, whose sorted
    /// position is immediately after the leaf at `after`. Prefers the gap at
    /// `after + 1` (O(1), fully adjacent to the parent — no bounce); if that is
    /// occupied (the parent is an overflow leaf sitting at an odd slot whose
    /// neighbors are live), forward-scans to the nearest free gap, wrapping to
    /// the front if none lies ahead. The disorder this introduces is bounded
    /// and local (a gap or two over), never arena-spanning.
    ///
    /// The caller (`split_leaf`) guarantees a free gap exists — it `spread`s
    /// first when the arena is full — so this never mutates and always finds a
    /// slot. A gap is an empty `LeafNode`: `keys.len() == 0`.
    fn claim_slot(&self, after: usize) -> usize {
        let n = self.leaves.len();
        // Forward scan from after+1 to the end.
        let mut i = after + 1;
        while i < n {
            if self.leaves[i].keys.len() == 0 {
                return i;
            }
            i += 1;
        }
        // Wrap: the only remaining gap may sit before `after`.
        for i in 0..after {
            if self.leaves[i].keys.len() == 0 {
                return i;
            }
        }
        // `after` itself is live (it's the leaf being split); no slot at `after`.
        unreachable!("claim_slot: no free gap (caller must spread when full)")
    }

    /// Walk internal nodes to find the leaf index for `needle`, allocating the
    /// descent `path` (`[(inode_idx, pos)]` from root to parent) so callers that
    /// need to propagate a split back up the tree can do so. `insert` uses this.
    /// Uses `height` to know when we've reached the leaf level — after
    /// `height` hops through inodes, the final pointer is a leaf index.
    ///
    /// **Rebalances on the way down.** Every full inode we pass through, and the
    /// target leaf if it is full, is redistributed with an emptier sibling *now*
    /// — before we descend further. This guarantees every node on the path has
    /// room by the time `insert` runs, so a leaf split's separator insertion
    /// into the bottom inode does not cascade, and the leaf usually has room for
    /// the new key without splitting at all. Splits still occur (via the
    /// unchanged `split_*` fallback) when a node and all its siblings are full.
    ///
    /// After an inode rebalance the node's key range shifts, so the needle may
    /// now belong to the sibling we rebalanced with; we re-run `find_child` on
    /// the grandparent (its separator was just updated) to re-route. The same
    /// re-route after the leaf rebalance ensures a pre-existing key that the
    /// rebalance moved to a sibling is still found (so the duplicate check in
    /// `insert` lands on the right leaf).
    fn walk_to_leaf(&mut self, needle: &SK::Needle) -> (usize, Vec<(usize, usize)>) {
        if self.height == 0 {
            return (0, Vec::new());
        }
        let mut path = Vec::new();
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle);
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            // Preemptive rebalance: if the child inode is full, redistribute it
            // with an emptier sibling before descending into it. Re-route only
            // if a redistribution actually happened (the grandparent separator
            // moved); the common case is no rebalance, so we skip the second
            // find_child/get_ptr entirely.
            let mut child = child;
            if self.inodes[child_idx].would_split() && self.try_rebalance_inode(node_idx, child) {
                child = self.inodes[node_idx].find_child(needle);
            }
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            path.push((node_idx, child));
            node_idx = child_idx;
        }
        // Last hop: the pointer from the bottom inode is a leaf index.
        let child = self.inodes[node_idx].find_child(needle);
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        // Same preemptive rebalance for the leaf. Re-route only if a
        // redistribution happened (the needle may now belong to the sibling
        // leaf); otherwise the original routing is still valid.
        let mut child = child;
        if self.leaves[leaf_idx].would_split() && self.try_rebalance_leaf(node_idx, child) {
            child = self.inodes[node_idx].find_child(needle);
        }
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        path.push((node_idx, child));
        (leaf_idx, path)
    }

    /// Read-only leaf lookup for `needle` — the allocation-free counterpart to
    /// `walk_to_leaf`. Used by `get`/`get_mut`/`cursor_at`, which discard the
    /// descent path and so must not pay to build one.
    #[inline]
    fn find_leaf(&self, needle: &SK::Needle) -> usize {
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

    // -----------------------------------------------------------------
    // Preemptive rebalance (rotate before split) — see `walk_to_leaf`.
    //
    // Each `try_rebalance_*` picks the emptier adjacent sibling of a full node
    // and, if redistributing leaves both nodes with room, moves entries to
    // balance them. `redistribute_*_{left,right}` do the in-place move and fix
    // the parent separator. The full node always receives the smaller half, so
    // it ends with `<= N-1` entries (room). The guard `s + 2 <= N` (sibling has
    // >= 2 free slots) ensures BOTH nodes end `<= N-1`: since the incoming
    // insert may route to either node after re-route, both must have room. When
    // the sibling has exactly one free slot (`s == N-1`), redistribution would
    // fill the sibling and risk an overflow insert into it, so we skip and let
    // the split fallback handle it.
    // -----------------------------------------------------------------

    /// Does the leaf at `idx` have >= 2 free slots? Required so both nodes end
    /// <= N-1 after rebalance (the `s + 2 <= N` invariant).
    #[inline]
    fn leaf_has_room_for_two(&self, idx: usize) -> bool {
        self.leaves[idx].keys.len() + 2 <= N
    }

    /// Does the inode at `idx` have >= 2 free slots? Required so both nodes end
    /// <= N-1 after rebalance (the `s + 2 <= N` invariant).
    #[inline]
    fn inode_has_room_for_two(&self, idx: usize) -> bool {
        self.inodes[idx].keys.len() + 2 <= N
    }

    /// Pick the emptier adjacent sibling of the full child at
    /// `parent.ptrs[child_pos]` that also has >= 2 free slots. Returns
    /// `(node_idx, sib_idx, pick_right)` or `None`. `sib_fill` maps a sibling
    /// arena index to its current key count; `has_room_for_two` is the
    /// `s + 2 <= N` guard for that arena. This is the shared core of
    /// `try_rebalance_leaf` / `try_rebalance_inode`; the arena lookup and
    /// dispatched `redistribute_*` differ per caller.
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
        // Pick the emptier adjacent sibling (tie → right). A missing sibling is
        // treated as full, so a present one always wins.
        let pick_right = match (left, right) {
            (Some(l), Some(r)) => sib_fill(r) <= sib_fill(l),
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => return None,
        };
        let sib = if pick_right { right } else { left }.unwrap();
        // Guard: both nodes must end with room after rebalance, so the sibling
        // needs >= 2 free slots (`s + 2 <= N`). With only one free slot the
        // sibling would fill and the re-routed insert could overflow it; skip
        // and let the split fallback handle it.
        if !has_room_for_two(sib) {
            return None;
        }
        Some((node_idx, sib, pick_right))
    }

    /// Pick the emptier sibling leaf of the full leaf at `parent.ptrs[child_pos]`
    /// and rebalance with it if both leaves can end with room. Delegates the
    /// shared sibling selection + guard to `pick_rebalance_sibling`. Returns
    /// `true` iff a redistribution was performed, `false` when no sibling was
    /// available or the `s + 2 <= N` guard failed.
    fn try_rebalance_leaf(&mut self, parent_idx: usize, child_pos: usize) -> bool {
        let Some((leaf_idx, sib, pick_right)) = self.pick_rebalance_sibling(
            parent_idx,
            child_pos,
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

    /// Rebalance full leaf `leaf_idx` (at `child_pos`) with its RIGHT sibling
    /// `sib_idx`: move the leaf's largest `m` keys/values to the sibling's front.
    /// `parent.keys[child_pos]` becomes the sibling's new first key.
    fn redistribute_leaf_right(
        &mut self,
        parent_idx: usize,
        child_pos: usize,
        leaf_idx: usize,
        sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s); // keys the full leaf keeps (the smaller half)
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_into_front(l_target, &mut sib.keys);
            leaf.values.drain_into_front(l_target, &mut sib.values);
        }
        // Leaf separator = right child's first key (B+ tree invariant).
        let new_sep = self.leaves[sib_idx].keys.get(0).clone();
        *self.inodes[parent_idx].keys.get_mut(child_pos) = new_sep;
    }

    /// Rebalance full leaf `leaf_idx` (at `child_pos`) with its LEFT sibling
    /// `sib_idx`: move the leaf's smallest `m` keys/values to the sibling's end.
    /// `parent.keys[child_pos - 1]` becomes the leaf's new first key.
    fn redistribute_leaf_left(
        &mut self,
        parent_idx: usize,
        child_pos: usize,
        leaf_idx: usize,
        sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s);
        let m = N - l_target; // keys moved from the full leaf's front to the sibling
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_front_into(m, &mut sib.keys);
            leaf.values.drain_front_into(m, &mut sib.values);
        }
        let new_sep = self.leaves[leaf_idx].keys.get(0).clone();
        *self.inodes[parent_idx].keys.get_mut(child_pos - 1) = new_sep;
    }

    /// Pick the emptier sibling inode of the full inode at
    /// `gparent.ptrs[child_pos]` and rebalance with it if both can end with
    /// room. Delegates the shared sibling selection + guard to
    /// `pick_rebalance_sibling`. Returns `true` iff a redistribution was
    /// performed, `false` when no sibling was available or the `s + 2 <= N`
    /// guard failed.
    fn try_rebalance_inode(&mut self, gparent_idx: usize, child_pos: usize) -> bool {
        let Some((l_idx, sib, pick_right)) = self.pick_rebalance_sibling(
            gparent_idx,
            child_pos,
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

    /// Rebalance full inode `l_idx` (at `child_pos`) with its RIGHT sibling
    /// `r_idx`, threading the grandparent separator `sep0 = gparent.keys[pos]`
    /// down into the sibling and lifting `L.keys[l_target]` up as the new sep.
    ///
    /// After: `L.keys = L[0..l_target]`, `L.ptrs = L[0..=l_target]`;
    /// `R.keys = [L[l_target+1..N], sep0, R[0..s]]`, `R.ptrs = [L[l_target+1..=N],
    /// R[0..=s]]`; `gparent.keys[pos] = L.keys[l_target]`.
    fn redistribute_inode_right(
        &mut self,
        gparent_idx: usize,
        child_pos: usize,
        l_idx: usize,
        r_idx: usize,
    ) {
        // Read the sibling fill and the current separator before any mutation.
        let (s, sep0) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[r_idx].keys.len(), g.keys.get(child_pos).clone())
        };
        let l_target = Self::rebalance_target(s); // keys the full inode keeps (smaller half)
        let m = N - l_target; // keys prepended to R (== ptrs prepended to R)

        let new_sep = {
            let (l, r) = two_mut(&mut self.inodes, l_idx, r_idx);
            // Prepend m ptrs to R: shift R's live ptrs [0..=s] right by m, then
            // take L's tail ptrs [l_target+1..=N] into R's front.
            // copy_within is a memmove (overlap-safe in both directions), so the
            // direction no longer matters here.
            r.ptrs.copy_within(0..=s, m);
            for i in 0..m {
                r.ptrs[i] = l.ptrs[l_target + 1 + i].take();
            }
            // Prepend m keys to R: move L's keys [l_target+1..N] (m-1 of them,
            // when m > 1) to R's front, then insert sep0 at R[m-1].
            if m > 1 {
                l.keys.drain_into_front(l_target + 1, &mut r.keys);
            }
            r.keys.insert_at(m - 1, sep0);
            // Lift the new separator off L's tail (now its last key).
            l.keys.remove_at(l_target)
        };
        // L's ptrs [l_target+1..=N] were taken (None) above; live ptrs are now
        // [0..=l_target], matching L.keys.len() == l_target. Nothing to clear.
        *self.inodes[gparent_idx].keys.get_mut(child_pos) = new_sep;
    }

    /// Rebalance full inode `l_idx` (at `child_pos`) with its LEFT sibling
    /// `sib_idx`, threading `sep0 = gparent.keys[child_pos - 1]` down into the
    /// sibling and lifting `L.keys[m-1]` up as the new sep.
    ///
    /// After: `L.keys = L[m..N]`, `L.ptrs = L[m..=N]`;
    /// `S.keys = [S[0..s], sep0, L[0..m-1]]`, `S.ptrs = [S[0..=s], L[0..m]]`;
    /// `gparent.keys[child_pos - 1] = L.keys[m-1]`.
    fn redistribute_inode_left(
        &mut self,
        gparent_idx: usize,
        child_pos: usize,
        l_idx: usize,
        sib_idx: usize,
    ) {
        let (s, sep0) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[sib_idx].keys.len(), g.keys.get(child_pos - 1).clone())
        };
        let l_target = Self::rebalance_target(s);
        let m = N - l_target; // keys appended to S (== ptrs appended to S)

        let new_sep = {
            let (l, sib) = two_mut(&mut self.inodes, l_idx, sib_idx);
            // Append m ptrs to S: take L's front ptrs [0..m] into S's tail.
            for i in 0..m {
                sib.ptrs[(s + 1) + i] = l.ptrs[i].take();
            }
            // Append m keys to S: sep0 first, then L's front keys [0..m-1].
            sib.keys.push(sep0);
            if m > 1 {
                l.keys.drain_front_into(m - 1, &mut sib.keys);
            }
            // Lift the new separator = L's current first key (original L[m-1]).
            l.keys.remove_at(0)
        };
        // Shift L's remaining ptrs [m..=N] down to [0..=N-m]; clear the tail.
        {
            let l = &mut self.inodes[l_idx];
            // copy_within is a memmove (overlap-safe in both directions), so the
            // direction no longer matters here.
            l.ptrs.copy_within(m..=N, 0);
            for i in (N - m + 1)..=N {
                l.ptrs[i] = None;
            }
        }
        *self.inodes[gparent_idx].keys.get_mut(child_pos - 1) = new_sep;
    }

    /// Shared leaf+position lookup for the read-side query methods. Returns
    /// `(leaf_idx, pos)` where `pos` is the lower bound of `needle` in the leaf
    /// — the same pair `get`/`get_mut`/`cursor_at` all need. The caller performs
    /// the form-specific exact-hit check (`SK::eq_key`) if it needs to confirm.
    #[inline]
    fn locate(&self, needle: &SK::Needle) -> (usize, usize) {
        let leaf_idx = self.find_leaf(needle);
        let pos = self.leaves[leaf_idx].find_position(needle);
        (leaf_idx, pos)
    }

    /// Search for `key` and return a reference to the value if found.
    ///
    /// `key` is anything that can be borrowed as the canonical lookup needle
    /// (`K` for the fixed tree, `[K]` for the variable tree). This is the
    /// consumer conversion seam: it carries the `Borrow` equivalence contract
    /// that makes sorted lookup sound.
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        Q: Borrow<SK::Needle>,
    {
        if self.leaves.is_empty() {
            return None;
        }
        let needle = key.borrow();
        let (leaf_idx, pos) = self.locate(needle);
        let leaf = &self.leaves[leaf_idx];
        if pos < leaf.keys.len() && SK::eq_key(leaf.keys.get(pos), needle) {
            return Some(leaf.values.get(pos));
        }
        None
    }

    /// Search for `key` and return a mutable reference to the value if found.
    pub fn get_mut<Q: ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Borrow<SK::Needle>,
    {
        if self.leaves.is_empty() {
            return None;
        }
        let needle = key.borrow();
        let (leaf_idx, pos) = self.locate(needle);
        let leaf = &mut self.leaves[leaf_idx];
        if pos < leaf.keys.len() && SK::eq_key(leaf.keys.get(pos), needle) {
            return Some(leaf.values.get_mut(pos));
        }
        None
    }

    /// Insert a key-value pair. Returns `Err((key, value))` if the key already
    /// exists.
    ///
    /// The key is taken in the canonical stored form (`K` for the fixed tree,
    /// `Box<[K]>` for the variable tree). The consumer conversion seam lives on
    /// the lookup side (`get`/`get_mut`/`cursor_at` via `Borrow`); insertion
    /// takes the owned canonical form so that fixed-tree literal inference
    /// (`insert(10, v)`) is preserved and variable-tree callers hand a
    /// `Box<[K]>` directly.
    pub fn insert(&mut self, key: SK, value: V) -> Result<(), (SK, V)> {
        let _ = Self::ASSERT_N_FITS;

        let (child_idx, path) = self.walk_to_leaf(key.borrow());
        let leaf = &self.leaves[child_idx];
        let pos = leaf.find_position(key.borrow());

        // Key already exists?
        if pos < leaf.keys.len() && leaf.keys.get(pos) == &key {
            return Err((key, value));
        }

        // If leaf is full, split first then determine which half the key lands in.
        // `split_leaf` returns the (possibly relocated) parent slot and the new
        // leaf's slot — a split may `spread` the arena, which moves every live
        // leaf to an even slot, so the indices we hold (`child_idx`) become
        // stale and must be taken from the return value.
        if leaf.keys.len() >= N {
            let mid = N / 2;
            let (parent_idx, new_leaf_idx) = self.split_leaf(child_idx, path);
            // After split: left leaf has keys[0..mid], right leaf has keys[mid..N].
            // Determine which leaf the key belongs to based on its position.
            if pos <= mid {
                self.leaves[parent_idx].insert(pos, key, value);
            } else {
                self.leaves[new_leaf_idx].insert(pos - mid, key, value);
            }
        } else {
            self.leaves[child_idx].insert(pos, key, value);
        }

        self.len += 1;
        Ok(())
    }

    /// Split the full leaf at `child_idx` into two halves and place the new
    /// right-half leaf in a free gap slot adjacent to its parent, then insert
    /// the separator into the parent. Returns `(parent_idx, new_leaf_idx)`: the
    /// (possibly relocated) arena slot of the leaf that was split, and the slot
    /// of the new right-half leaf. The caller needs both because a `spread`
    /// triggered here moves every live leaf.
    fn split_leaf(&mut self, child_idx: usize, mut path: Vec<(usize, usize)>) -> (usize, usize) {
        // Leaf is full (N keys). Split at mid = N/2.
        // Left keeps keys[0..mid], right gets keys[mid..N].
        // Separator key (keys[mid]) goes to parent and also stays in right leaf.
        let mid = N / 2;
        let mid_key = self.leaves[child_idx].keys.get(mid).clone();

        // If the arena is full (no free gap slots), spread first to recreate
        // gaps. `spread` relocates every live leaf to an even slot, so re-derive
        // `child_idx` through the returned old→new map. Done before any mutation
        // so the rest of the split works in a single, stable index space.
        let child_idx = if self.n_leaves == self.leaves.len() {
            let map = self.spread();
            map[child_idx]
        } else {
            child_idx
        };

        // Save linked-list state before modifying (read fresh: `spread` may
        // have rewired it, but the parent's next still names the same successor
        // leaf, now at its relocated slot).
        let old_next = self.leaves[child_idx].get_next();

        // Create new leaf with upper half, then truncate the old leaf.
        let mut new_leaf = LeafNode::<SK, V, PTR, N>::new();
        self.leaves[child_idx].keys.drain_into(mid, &mut new_leaf.keys);
        self.leaves[child_idx].values.drain_into(mid, &mut new_leaf.values);

        // Claim a free gap slot adjacent to the parent (prefers child_idx + 1).
        let new_leaf_idx = self.claim_slot(child_idx);

        // Wire up leaf linked list: old_leaf <-> new_leaf <-> old_next. Set the
        // links on the local `new_leaf` before moving it into its slot.
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

        // Insert separator key into parent, or create new root.
        self.insert_separator(mid_key, new_leaf_idx, &mut path);
        (child_idx, new_leaf_idx)
    }

    fn insert_separator(&mut self, key: SK, new_child_idx: usize, path: &mut Vec<(usize, usize)>) {
        if path.is_empty() {
            // Need a new root inode. The new root's children are the old root
            // (which was either a leaf or an inode) and the new child.
            let old_root_idx = self.root_inode;
            let mut root = KeyNode::<SK, PTR, N, NP1>::new();
            root.keys.insert_at(0, key);
            root.set_ptr(0, old_root_idx);
            root.set_ptr(1, new_child_idx);
            let root_idx = self.inodes.len();
            self.inodes.push(root);
            self.root_inode = root_idx;
            self.height += 1;
            return;
        }

        // Pop the parent inode from the path
        let (parent_idx, _) = path.pop().unwrap();

        if !self.inodes[parent_idx].would_split() {
            // Room in parent — just insert
            let parent = &mut self.inodes[parent_idx];
            let pos = parent.insert_leaf(key);
            parent.set_ptr(pos + 1, new_child_idx);
        } else {
            // Parent is full — split it
            self.split_inode(parent_idx, key, new_child_idx, path);
        }
    }

    /// Split a full internal node.
    ///
    /// A full node (`n == N` keys, `n+1` ptrs) receives a new `(key, child)`
    /// pair, giving `n+1` keys and `n+2` ptrs — one too many. We split at
    /// `mid = n/2` and push the median up to the parent:
    ///
    ///   left  := keys[0..mid),     ptrs[0..=mid]
    ///   sep   := the median key   (pushed up, not retained in either child)
    ///   right := keys[mid+1..n),  ptrs[mid+1..=n]
    ///
    /// plus the new pair spliced into whichever half its position `pos` falls
    /// in. The separator is extracted as the tail of the appropriate range so
    /// no `remove_at(0)` (front shift) is ever needed:
    ///
    /// - `pos == mid`: the new key *is* the median, so it goes straight up.
    ///   Old `keys[mid..n)` all move to right (drain from `mid`), and the new
    ///   child becomes right's leftmost ptr.
    /// - `pos != mid`: old `keys[mid]` is the separator. We drain `keys[mid+1..n)`
    ///   into right (skipped when that range is empty — e.g. `N == 2`), which
    ///   leaves `keys[mid]` sitting at left's tail; `pop()` lifts it off with
    ///   no shifting.
    fn split_inode(
        &mut self,
        parent_idx: usize,
        new_key: SK,
        new_child_idx: usize,
        path: &mut Vec<(usize, usize)>,
    ) {
        let pos = self.inodes[parent_idx].find_position(new_key.borrow());
        let n = self.inodes[parent_idx].keys.len(); // == N (full)
        let mid = n / 2;
        let right_half = n - mid; // ptrs moved from left to right

        let mut right = KeyNode::<SK, PTR, N, NP1>::new();

        // Move the upper key half to right, then move the matching upper ptrs
        // [mid+1..=n] across; left retains ptrs[0..=mid] (the separator's left
        // subtree and below). For pos==mid the new child claims right's slot 0,
        // so the moved ptrs land at [1..=right_half] in the same pass.
        {
            let left = &mut self.inodes[parent_idx];
            if pos == mid {
                left.keys.drain_into(mid, &mut right.keys);
                right.set_ptr(0, new_child_idx);
                for i in 0..right_half {
                    right.ptrs[i + 1] = left.ptrs[mid + 1 + i].take();
                }
            } else {
                // Drain keys above the separator only; when right_half == 1
                // (e.g. N==2) there are none, so skip the drain entirely.
                if right_half > 1 {
                    left.keys.drain_into(mid + 1, &mut right.keys);
                }
                for i in 0..right_half {
                    right.ptrs[i] = left.ptrs[mid + 1 + i].take();
                }
            }
        }

        // Extract the separator and splice the new pair into its half. For
        // pos==mid the key is consumed as the separator, so only the child
        // pointer (already placed above) is needed. Otherwise the separator
        // is old keys[mid], now at left's tail — pop it, no front shift.
        let separator = if pos == mid {
            new_key
        } else {
            let sep = self.inodes[parent_idx]
                .keys
                .pop()
                .expect("split_inode: left must retain the separator");
            if pos < mid {
                let left = &mut self.inodes[parent_idx];
                left.insert_key_at(pos, new_key);
                left.set_ptr(pos + 1, new_child_idx);
            } else {
                let rpos = pos - mid - 1;
                right.insert_key_at(rpos, new_key);
                right.set_ptr(rpos + 1, new_child_idx);
            }
            sep
        };

        let right_idx = self.inodes.len();
        self.inodes.push(right);
        self.insert_separator(separator, right_idx, path);
    }

    /// Descend from the root to a leaf, following either the leftmost child
    /// (`rightmost == false`) or the rightmost child (`rightmost == true`) at
    /// every inode. The shared core of `first_leaf` / `last_leaf`.
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

    /// Walk down the leftmost path to find the first (smallest-key) leaf.
    fn first_leaf(&self) -> usize {
        self.descend_to_leaf(false)
    }

    /// Walk down the rightmost path to find the last (largest-key) leaf.
    fn last_leaf(&self) -> usize {
        self.descend_to_leaf(true)
    }

    /// Create a cursor positioned at the first element.
    pub fn get_cursor(&self) -> Cursor<SK, V, PTR, N, NP1> {
        let leaf_idx = self.first_leaf();
        Cursor {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    /// Create a mutable cursor positioned at the first element.
    pub fn get_cursor_mut(&mut self) -> CursorMut<SK, V, PTR, N, NP1> {
        let leaf_idx = self.first_leaf();
        CursorMut {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    /// Create a cursor positioned at the key (or the nearest key >= it).
    pub fn cursor_at<Q: ?Sized>(&self, key: &Q) -> Cursor<SK, V, PTR, N, NP1>
    where
        Q: Borrow<SK::Needle>,
    {
        let (leaf_idx, pos) = self.locate(key.borrow());
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

impl<'a, SK, V, PTR, const N: usize, const NP1: usize> Cursor<'a, SK, V, PTR, N, NP1>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    /// Return the current key-value pair, or None if the cursor is exhausted.
    pub fn current(&self) -> Option<(&SK, &V)> {
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get(self.position)))
        } else {
            None
        }
    }

    /// Advance to the next key-value pair and return the value, or None if exhausted.
    pub fn next(&mut self) -> Option<&V> {
        // Try to advance within the current leaf
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position + 1 < leaf.keys.len() {
            self.position += 1;
            return Some(self.tree.leaves[self.leaf_idx].values.get(self.position));
        }
        // Move to the next leaf
        let next_leaf = leaf.get_next()?;
        self.leaf_idx = next_leaf;
        self.position = 0;
        Some(self.tree.leaves[self.leaf_idx].values.get(0))
    }

    /// Move to the previous key-value pair and return the value, or None if at the beginning.
    pub fn prev(&mut self) -> Option<&V> {
        // Try to move back within the current leaf
        if self.position > 0 {
            self.position -= 1;
            return Some(self.tree.leaves[self.leaf_idx].values.get(self.position));
        }
        // Move to the previous leaf
        let prev_leaf = self.tree.leaves[self.leaf_idx].get_prev()?;
        self.leaf_idx = prev_leaf;
        let last_pos = self.tree.leaves[self.leaf_idx].keys.len() - 1;
        self.position = last_pos;
        Some(self.tree.leaves[self.leaf_idx].values.get(last_pos))
    }
}

impl<'a, SK, V, PTR, const N: usize, const NP1: usize> CursorMut<'a, SK, V, PTR, N, NP1>
where
    SK: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); NP1]:
{
    /// Return the current key-value pair (mutable ref on value), or None if exhausted.
    pub fn current(&mut self) -> Option<(&SK, &mut V)> {
        let leaf = &mut self.tree.leaves[self.leaf_idx];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get_mut(self.position)))
        } else {
            None
        }
    }

    /// Advance to the next key-value pair and return a mutable ref to the value,
    /// or None if exhausted.
    pub fn next(&mut self) -> Option<&mut V> {
        // Try to advance within the current leaf
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position + 1 < leaf.keys.len() {
            self.position += 1;
        } else {
            // Move to the next leaf
            let next_leaf = leaf.get_next();
            if let Some(nl) = next_leaf {
                self.leaf_idx = nl;
                self.position = 0;
            } else {
                // No more leaves — exhausted
                return None;
            }
        }
        Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position))
    }

    /// Move to the previous key-value pair and return a mutable ref to the value.
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
pub type FixedCTree<K, V, PTR, const N: usize, const NP1: usize> = CTree<K, V, PTR, N, NP1>;

/// Variable-length-key B+ tree (binary search). Keys are `Box<[K]>`.
pub type VarCTree<K, V, PTR, const N: usize, const NP1: usize> = CTree<Box<[K]>, V, PTR, N, NP1>;

#[cfg(test)]
#[path = "tests/tiny_btree.rs"]
mod tests;