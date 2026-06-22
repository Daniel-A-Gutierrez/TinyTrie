use std::cmp::Ordering;
use std::marker::PhantomData;
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
    ///
    /// Default: scalar linear scan. Overridden by SIMD impls.
    fn find_position(needle: &Self, haystack: &[Self]) -> usize {
        for i in 0..haystack.len() {
            if haystack[i] >= *needle {
                return i;
            }
        }
        haystack.len()
    }
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
// Fixed-len node types
// ---------------------------------------------------------------------------

struct KeyNode<K, PTR: TrieIndex, const N: usize>
where
    K: FixedLenKey,
    [(); N + 1]:
{
    keys: TinyArray<K, N>,
    ptrs: [Option<NonZero<PTR>>; N + 1],
}

struct LeafNode<K, V, const N: usize>
where
    K: FixedLenKey,
    V: Sized,
    [(); N]:
{
    keys: TinyArray<K, N>,
    values: TinyArray<V, N>,
}

// ---------------------------------------------------------------------------
// Variable-len node types
// ---------------------------------------------------------------------------

struct VarKeyNode<K, PTR: TrieIndex, const N: usize>
where
    K: FixedLenKey,
    [(); N + 1]:
{
    keys: TinyArray<Box<[K]>, N>,
    ptrs: [Option<NonZero<PTR>>; N + 1],
}

struct VarLeafNode<K, V, const N: usize>
where
    K: FixedLenKey,
    V: Sized,
    [(); N]:
{
    keys: TinyArray<Box<[K]>, N>,
    values: TinyArray<V, N>,
}

// ---------------------------------------------------------------------------
// Fixed-len CTree (B+ tree for Copy keys — SIMD search)
// ---------------------------------------------------------------------------

/// B+ tree for fixed-size keys with SIMD-accelerated search.
struct CTree<K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    inodes: Vec<KeyNode<K, PTR, N>>,
    leaves: Vec<LeafNode<K, V, N>>,
    len: usize,
}

struct Cursor<'a, K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    tree: &'a CTree<K, V, PTR, N>,
    stack: Vec<usize>,
    position: usize,
    phantom: PhantomData<V>,
}

struct CursorMut<'a, K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    tree: &'a mut CTree<K, V, PTR, N>,
    stack: Vec<usize>,
    position: usize,
    phantom: PhantomData<V>,
}

// ---------------------------------------------------------------------------
// Variable-len VarCTree (B+ tree for VarLenKey — binary search)
// ---------------------------------------------------------------------------

/// B+ tree for variable-length keys with binary-search comparison.
struct VarCTree<K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    inodes: Vec<VarKeyNode<K, PTR, N>>,
    leaves: Vec<VarLeafNode<K, V, N>>,
    len: usize,
}

struct VarCursor<'a, K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    tree: &'a VarCTree<K, V, PTR, N>,
    stack: Vec<usize>,
    position: usize,
    phantom: PhantomData<V>,
}

struct VarCursorMut<'a, K, V, PTR, const N: usize>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    tree: &'a mut VarCTree<K, V, PTR, N>,
    stack: Vec<usize>,
    position: usize,
    phantom: PhantomData<V>,
}

// ---------------------------------------------------------------------------
// KeyNode impl
// ---------------------------------------------------------------------------

impl<K, PTR: TrieIndex, const N: usize> KeyNode<K, PTR, N>
where
    K: FixedLenKey,
    [(); N + 1]:
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            ptrs: [None; N + 1],
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
    fn get(&self, i: usize) -> &K {
        self.keys.get(i)
    }

    /// Get key at index `i` without bounds check.
    #[inline]
    unsafe fn get_unchecked(&self, i: usize) -> &K {
        self.keys.get_unchecked(i)
    }

    /// Get child pointer at index `i`. Returns `None` for empty slots.
    #[inline]
    fn get_ptr(&self, i: usize) -> Option<PTR> {
        debug_assert!(i <= self.keys.len());
        self.ptrs[i].map(|nz| nz.get())
    }

    /// Get child pointer at index `i` without bounds check.
    #[inline]
    unsafe fn get_ptr_unchecked(&self, i: usize) -> Option<PTR> {
        self.ptrs[i].map(|nz| nz.get())
    }

    #[inline]
    fn find_position(&self, k: &K) -> usize {
        K::find_position(k, self.keys.as_slice())
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

    /// Insert `k` into this internal node in sorted order.
    /// Also shifts ptrs. Caller guarantees `!would_split()`.
    /// Returns the position where the key was inserted.
    fn insert_leaf(&mut self, k: K) -> usize {
        debug_assert!(!self.would_split());
        let pos = self.find_position(&k);
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, k);
        pos
    }

    /// Remove key at `pos` and its right child pointer.
    /// Returns the removed key.
    fn remove(&mut self, pos: usize) -> K {
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

impl<K, V, const N: usize> LeafNode<K, V, N>
where
    K: FixedLenKey,
    V: Sized,
    [(); N]:
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            values: TinyArray::new(),
        }
    }

    fn find_position(&self, k: &K) -> usize {
        K::find_position(k, self.keys.as_slice())
    }

    /// Insert key-value at position `pos`. Caller must ensure pos is correct
    /// (from `find_position`) and node is not full.
    fn insert(&mut self, pos: usize, k: K, v: V) {
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
    }

    fn remove(&mut self, pos: usize) -> (K, V) {
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
// VarKeyNode impl
// ---------------------------------------------------------------------------

impl<K, PTR: TrieIndex, const N: usize> VarKeyNode<K, PTR, N>
where
    K: FixedLenKey,
    [(); N + 1]:
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            ptrs: [None; N + 1],
        }
    }

    #[inline]
    fn get(&self, i: usize) -> &Box<[K]> {
        self.keys.get(i)
    }

    #[inline]
    unsafe fn get_unchecked(&self, i: usize) -> &Box<[K]> {
        self.keys.get_unchecked(i)
    }

    #[inline]
    fn get_ptr(&self, i: usize) -> Option<PTR> {
        debug_assert!(i <= self.keys.len());
        self.ptrs[i].map(|nz| nz.get())
    }

    #[inline]
    unsafe fn get_ptr_unchecked(&self, i: usize) -> Option<PTR> {
        self.ptrs[i].map(|nz| nz.get())
    }

    #[inline]
    fn find_position(&self, k: &[K]) -> usize {
        let n = self.keys.len();
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let node_key = self.keys.get(mid);
            match node_key.as_ref().cmp(k) {
                Ordering::Less => lo = mid + 1,
                Ordering::Equal => return mid,
                Ordering::Greater => hi = mid,
            }
        }
        lo
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    #[inline]
    fn would_merge(&self) -> bool {
        self.keys.len() == N / 2
    }

    fn insert_leaf(&mut self, k: Box<[K]>) -> usize {
        debug_assert!(!self.would_split());
        let pos = self.find_position(&k);
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, k);
        pos
    }

    fn remove(&mut self, pos: usize) -> Box<[K]> {
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
// VarLeafNode impl
// ---------------------------------------------------------------------------

impl<K, V, const N: usize> VarLeafNode<K, V, N>
where
    K: FixedLenKey,
    V: Sized,
    [(); N]:
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            values: TinyArray::new(),
        }
    }

    fn find_position(&self, k: &[K]) -> usize {
        let n = self.keys.len();
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let node_key = self.keys.get(mid);
            match node_key.as_ref().cmp(k) {
                Ordering::Less => lo = mid + 1,
                Ordering::Equal => return mid,
                Ordering::Greater => hi = mid,
            }
        }
        lo
    }

    fn insert(&mut self, pos: usize, k: Box<[K]>, v: V) {
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
    }

    fn remove(&mut self, pos: usize) -> (Box<[K]>, V) {
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
// CTree impl (fixed-len B+ tree)
// ---------------------------------------------------------------------------

impl<K, V, PTR, const N: usize> CTree<K, V, PTR, N>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");

    pub fn new() -> Self {
        // Start with one empty leaf node (the root).
        let root = LeafNode::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Walk internal nodes to find the leaf index for `key`.
    /// Returns `(leaf_idx, path)` where path is `[(inode_idx, pos)]` from root to parent.
    fn walk_to_leaf(&self, key: &K) -> (usize, Vec<(usize, usize)>) {
        let mut path = Vec::new();
        if self.inodes.is_empty() {
            return (0, path);
        }
        let mut node_idx: usize = 0;
        loop {
            let node = &self.inodes[node_idx];
            let pos = node.find_position(key);
            let Some(ptr) = node.get_ptr(pos) else {
                return (0, path);
            };
            let next = ptr.as_usize();
            path.push((node_idx, pos));
            if next < self.inodes.len() {
                node_idx = next;
            } else {
                return (next - self.inodes.len(), path);
            }
        }
    }

    /// Search for `key` and return a reference to the value if found.
    pub fn get(&self, key: &K) -> Option<&V> {
        if self.leaves.is_empty() {
            return None;
        }
        let (leaf_idx, _) = self.walk_to_leaf(key);
        let leaf = &self.leaves[leaf_idx];
        let pos = leaf.find_position(key);
        if pos < leaf.keys.len() {
            if leaf.keys.get(pos) == key {
                return Some(leaf.values.get(pos));
            }
        }
        None
    }

    /// Search for `key` and return a mutable reference to the value if found.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if self.leaves.is_empty() {
            return None;
        }
        let (leaf_idx, _) = self.walk_to_leaf(key);
        let leaf = &mut self.leaves[leaf_idx];
        let pos = leaf.find_position(key);
        if pos < leaf.keys.len() {
            if leaf.keys.get(pos) == key {
                return Some(leaf.values.get_mut(pos));
            }
        }
        None
    }

    /// Insert a key-value pair. Returns `Err((key, value))` if the key already exists.
    pub fn insert(&mut self, key: K, value: V) -> Result<(), (K, V)> {
        let _ = Self::ASSERT_N_FITS;

        let (child_idx, path) = self.walk_to_leaf(&key);
        let leaf = &mut self.leaves[child_idx];
        let pos = leaf.find_position(&key);

        // Key already exists?
        if pos < leaf.keys.len() {
            if leaf.keys.get(pos) == &key {
                return Err((key, value));
            }
        }

        // Insert into leaf
        leaf.insert(pos, key, value);
        self.len += 1;

        // Handle leaf split if needed
        if leaf.keys.len() > N {
            self.split_leaf(child_idx, path);
        }

        Ok(())
    }

    fn split_leaf(&mut self, child_idx: usize, mut path: Vec<(usize, usize)>) {
        let mid = (N + 1) / 2;
        let mid_key = self.leaves[child_idx].keys.get(mid).clone();

        // Create new leaf with upper half
        let mut new_leaf = LeafNode::<K, V, N>::new();
        let leaf = &mut self.leaves[child_idx];
        let split_len = leaf.keys.len() - mid;
        for i in 0..split_len {
            unsafe {
                new_leaf.keys.insert_at(i, leaf.keys.read_slot(mid + i));
                new_leaf.values.insert_at(i, leaf.values.read_slot(mid + i));
            }
        }
        leaf.keys.truncate(mid as u8);
        leaf.values.truncate(mid as u8);

        let new_leaf_idx = self.leaves.len();
        self.leaves.push(new_leaf);

        // Insert separator key into parent, or create new root
        self.insert_separator(mid_key, new_leaf_idx, &mut path);
    }

    fn insert_separator(
        &mut self,
        key: K,
        new_child_idx: usize,
        path: &mut Vec<(usize, usize)>,
    ) {
        // Convert leaf index to absolute node index
        let new_ptr = self.inodes.len() + new_child_idx;

        if path.is_empty() {
            // Need a new root inode
            let mut root = KeyNode::<K, PTR, N>::new();
            root.keys.insert_at(0, key);

            let old_inode_count = self.inodes.len();
            root.ptrs[0] = NonZero::new(PTR::from_usize(old_inode_count));
            root.ptrs[1] = NonZero::new(PTR::from_usize(old_inode_count + new_child_idx));

            self.inodes.push(root);
            return;
        }

        // Pop the parent inode from the path
        let (parent_idx, _) = path.pop().unwrap();
        let parent = &mut self.inodes[parent_idx];

        if !parent.would_split() {
            // Room in parent — just insert
            let pos = parent.find_position(&key);
            let l = parent.keys.len();
            if pos < l {
                for i in (pos + 1..=l).rev() {
                    parent.ptrs[i + 1] = parent.ptrs[i];
                }
            }
            parent.keys.insert_at(pos, key);
            parent.ptrs[pos + 1] = NonZero::new(PTR::from_usize(new_ptr));
        } else {
            // Parent is full — split it too
            // (recursive split, simplified for now)
            todo!("recursive inode split not yet implemented");
        }
    }

    pub fn get_cursor(&self) -> Cursor<K, V, PTR, N> {
        Cursor {
            tree: self,
            stack: Vec::new(),
            position: 0,
            phantom: PhantomData,
        }
    }

    pub fn get_cursor_mut(&mut self) -> CursorMut<K, V, PTR, N> {
        CursorMut {
            tree: self,
            stack: Vec::new(),
            position: 0,
            phantom: PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// VarCTree impl (variable-len B+ tree)
// ---------------------------------------------------------------------------

impl<K, V, PTR, const N: usize> VarCTree<K, V, PTR, N>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");

    pub fn new() -> Self {
        let root = VarLeafNode::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ---------------------------------------------------------------------------
// Cursor impl (fixed-len)
// ---------------------------------------------------------------------------

impl<'a, K, V, PTR, const N: usize> Cursor<'a, K, V, PTR, N>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    pub fn current(&self) -> Option<(&K, &V)> {
        if self.tree.leaves.is_empty() {
            return None;
        }
        let leaf = &self.tree.leaves[0];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get(self.position)))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<&V> {
        todo!("proper cursor traversal")
    }

    pub fn prev(&mut self) -> Option<&V> {
        todo!("proper cursor traversal")
    }
}

impl<'a, K, V, PTR, const N: usize> CursorMut<'a, K, V, PTR, N>
where
    K: FixedLenKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:
    ,
    [(); N + 1]:
{
    pub fn current(&mut self) -> Option<(&K, &mut V)> {
        if self.tree.leaves.is_empty() {
            return None;
        }
        let leaf = &mut self.tree.leaves[0];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get_mut(self.position)))
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/tiny_btree.rs"]
mod tests;