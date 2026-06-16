//! Fixed-Length Nibble Trie — a fixed-fanout radix trie with fixed-length key slots.
//!
//! Like [`NibbleTrie`], each node has 16 child slots (one per nibble value 0–15).
//! The key difference is that keys are stored in fixed-length slots in `buf`,
//! eliminating the need for a separate index vector and the `offset` field per node.
//!
//! # Key Storage
//!
//! Each key occupies exactly `max_len` bytes in `buf`, zero-padded on the right.
//! The key index `i` maps directly to `buf[i * max_len .. (i + 1) * max_len]`.
//! Actual key lengths are stored in a `lens: Vec<u16>` alongside `buf`, giving
//! O(1) key retrieval without an index vector. This adds only 2 bytes/key overhead
//! (vs NibbleTrie's ~10 bytes/key for `(usize, LEN)` on 64-bit).
//!
//! # Key Index
//!
//! Leaf key indices are 0-based (unlike NibbleTrie's 1-based scheme). The sentinel
//! value for empty child slots is `PTR::max_value()` (instead of 0). This gives
//! a max entry count of `PTR::max_value() - 1`.
//!
//! # Offset Elimination
//!
//! The `offset` field in NibbleTrie's `Node` is replaced by computing
//! `leaf.as_usize() * max_len` on demand. The terminal flag is stored in a
//! `flags: u8` field (bit 0).
//!
//! # Optimization
//!
//! [`optimize()`] rewrites `buf` and `values` in DFS-sorted order and remaps leaf
//! indices. Called automatically after each insert when `values.len()` is a power
//! of two (amortized O(1) per insert).

use crate::TinyTrieMap;
use std::simd::{LaneCount, Simd, SupportedLaneCount, cmp::SimdPartialEq};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Bit 0 of `FixedLenNode.flags` marks a terminal node (key ends here).
const TERMINAL_FLAG: u8 = 1;

// ---------------------------------------------------------------------------
// FixedLenNode
// ---------------------------------------------------------------------------

/// A single node in the fixed-length nibble trie arena.
///
/// Generic over `PTR` (pointer/index type for children and arena references).
/// Unlike `Node`, there is no `LEN` parameter (`prefix_len` is always `u16`)
/// and no `offset` field (computed from `leaf * max_len`).
///
/// Layout with PTR=u16: 40 bytes (saves 8 vs NibbleTrie's 48).
/// Layout with PTR=u32: 76 bytes (saves 4 vs NibbleTrie's 80).
#[derive(Copy, Clone)]
pub struct FixedLenNode<PTR: TrieIndex> {
    pub children: [PTR; 16],
    pub prefix_len: u16,
    pub leaf_mask: u16,
    pub leaf: PTR,
    pub flags: u8,
    // compiler pads to align(PTR)
}

impl<PTR: TrieIndex> FixedLenNode<PTR> {
    fn new() -> Self {
        FixedLenNode {
            children: [PTR::max_value_sentinel(); 16],
            prefix_len: 0,
            leaf_mask: 0,
            leaf: PTR::max_value_sentinel(),
            flags: 0,
        }
    }

    #[inline]
    pub fn is_terminal(&self) -> bool {
        self.flags & TERMINAL_FLAG != 0
    }

    #[inline]
    fn set_terminal(&mut self, val: bool) {
        if val {
            self.flags |= TERMINAL_FLAG;
        } else {
            self.flags &= !TERMINAL_FLAG;
        }
    }

    #[inline]
    pub fn is_leaf(&self, nib: usize) -> bool {
        debug_assert!(nib < 16);
        (self.leaf_mask >> nib) & 1 == 1
    }

    #[inline]
    fn set_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 16);
        self.leaf_mask |= 1 << nib;
    }

    #[inline]
    fn clear_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 16);
        self.leaf_mask &= !(1 << nib);
    }

    /// Store a leaf key index at `nib`. Key index must not equal the sentinel.
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, ki: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(ki != PTR::max_value_sentinel(), "key index max_value is sentinel");
        self.set_leaf(nib);
        self.children[nib] = ki;
    }

    /// Store an arena index at `nib` (internal node reference).
    /// Arena index must not equal the sentinel.
    #[inline]
    fn set_internal_child(&mut self, nib: usize, arena_idx: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(arena_idx != PTR::max_value_sentinel(), "arena index max_value is sentinel");
        self.clear_leaf(nib);
        self.children[nib] = arena_idx;
    }

    /// Decode a leaf child at `nib` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize) -> Option<PTR> {
        debug_assert!(nib < 16);
        if self.is_leaf(nib) && self.children[nib] != PTR::max_value_sentinel() {
            Some(self.children[nib])
        } else {
            None
        }
    }

    /// Compute a 16-bit mask where bit N is set if `children[N]` is occupied
    /// (either leaf or internal — not the empty sentinel).
    #[inline]
    pub fn children_mask(&self) -> u16 {
        // Invert children, then check for nonzero: XOR with max_value turns
        // sentinel into 0 and real values into nonzero.
        let mut mask = 0u16;
        for i in 0..16 {
            if self.children[i] != PTR::max_value_sentinel() {
                mask |= 1 << i;
            }
        }
        mask
    }
}

impl<PTR: TrieIndex> std::fmt::Debug for FixedLenNode<PTR> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active: Vec<(usize, &str, PTR)> = (0..16)
            .filter(|&n| self.children[n] != PTR::max_value_sentinel())
            .map(|n| {
                let tag = if self.is_leaf(n) { "L" } else { "I" };
                (n, tag, self.children[n])
            })
            .collect();
        f.debug_struct("FixedLenNode")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("0x{:04x}", self.leaf_mask))
            .field("leaf", &self.leaf)
            .field("terminal", &self.is_terminal())
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// TrieIndex extension
// ---------------------------------------------------------------------------

/// TrieIndex now provides `max_value_sentinel()` directly.
use crate::nibble_trie::TrieIndex;

// ---------------------------------------------------------------------------
// Nibble helpers (reuse from nibble_trie)
// ---------------------------------------------------------------------------

#[inline]
fn key_nibble_at(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 2;
    if byte_idx < key.len() {
        if idx % 2 == 0 {
            key[byte_idx] >> 4
        } else {
            key[byte_idx] & 0x0F
        }
    } else {
        0
    }
}

#[inline]
fn nibble_count(key: &[u8]) -> usize {
    key.len() * 2
}

#[inline]
fn diverging_nibble(xor: u8, byte_idx: usize) -> usize {
    byte_idx * 2 + ((xor >> 4 == 0) as usize)
}

// ---------------------------------------------------------------------------
// Divergence result
// ---------------------------------------------------------------------------

enum DivergeResult {
    Duplicate,
    At(usize),
}

#[inline]
fn find_divergence(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult {
    let total_a = nibble_count(key_a);
    let total_b = nibble_count(key_b);
    let min = total_a.min(total_b);
    let mut d = from;
    while d < min {
        if key_nibble_at(key_a, d) != key_nibble_at(key_b, d) {
            return DivergeResult::At(d);
        }
        d += 1;
    }
    if total_a == total_b {
        DivergeResult::Duplicate
    } else {
        DivergeResult::At(d)
    }
}

fn simd_find_divergence<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult
where
    LaneCount<N>: SupportedLaneCount,
{
    let minlen = key_a.len().min(key_b.len());
    let mut i = from / 2;

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor = unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            return DivergeResult::At(diverging_nibble(xor, diff_byte_idx));
        }
        i += N;
    }

    find_divergence(key_a, key_b, i * 2)
}

#[inline]
fn simd_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let len = a.len();
    let mut i = 0;
    while i + 16 <= len {
        let va = Simd::<u8, 16>::from_slice(unsafe { a.get_unchecked(i..i + 16) });
        let vb = Simd::<u8, 16>::from_slice(unsafe { b.get_unchecked(i..i + 16) });
        if va.simd_ne(vb).any() {
            return false;
        }
        i += 16;
    }
    while i < len {
        if unsafe { *a.get_unchecked(i) != *b.get_unchecked(i) } {
            return false;
        }
        i += 1;
    }
    true
}

// ---------------------------------------------------------------------------
// FixedLenNibbleTrie
// ---------------------------------------------------------------------------

/// A fixed-length nibble trie map.
///
/// Keys are stored in fixed-size slots of `max_len` bytes, zero-padded on the
/// right. Key length is recovered by scanning backward from the slot boundary
/// for the first nonzero byte. **Keys must not contain trailing zero bytes** —
/// a key like `b"a\0"` would be retrieved as `b"a"`.
///
/// The `PTR` type parameter controls the width of arena/key indices and thus
/// the maximum number of entries (~`PTR::max_value() - 1`).
#[derive(Clone)]
pub struct FixedLenNibbleTrie<T, PTR: TrieIndex = u32> {
    pub arena: Vec<FixedLenNode<PTR>>,
    pub buf: Vec<u8>,
    pub values: Vec<T>,
    pub lens: Vec<u16>,    // actual byte length of each key
    pub max_len: usize,
}

impl<T, PTR: TrieIndex> FixedLenNibbleTrie<T, PTR> {
    // -------------------------------------------------------------------
    // Construction & basic accessors
    // -------------------------------------------------------------------

    /// Create a new empty trie with the given maximum key length.
    ///
    /// Keys longer than `max_len` will be rejected by `insert`.
    pub fn new(max_len: usize) -> Self {
        FixedLenNibbleTrie {
            arena: Vec::new(),
            buf: Vec::new(),
            values: Vec::new(),
            lens: Vec::new(),
            max_len,
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    // -------------------------------------------------------------------
    // Key retrieval
    // -------------------------------------------------------------------

    /// Return the actual (unpadded) key slice for key index `ki`.
    /// Uses the stored length for O(1) retrieval.
    #[inline]
    pub fn key_slice(&self, ki: PTR) -> &[u8] {
        let idx = ki.as_usize();
        let start = idx * self.max_len;
        let len = self.lens[idx] as usize;
        &self.buf[start..start + len]
    }

    /// Return the full padded slot for key index `ki`.
    #[inline]
    fn padded_key(&self, ki: PTR) -> &[u8] {
        let start = ki.as_usize() * self.max_len;
        &self.buf[start..start + self.max_len]
    }

    /// Check whether the key at index `ki` matches `key`.
    /// Compares the stored key length first, then the bytes.
    #[inline]
    fn key_matches(&self, ki: PTR, key: &[u8]) -> bool {
        let idx = ki.as_usize();
        let len = self.lens[idx] as usize;
        if len != key.len() {
            return false;
        }
        let start = idx * self.max_len;
        self.buf[start..start + len] == *key
    }

    // -------------------------------------------------------------------
    // Lookup
    // -------------------------------------------------------------------

    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if key.len() > self.max_len || self.arena.is_empty() {
            return None;
        }
        let mut node_idx: PTR = PTR::zero();
        let max_nib = key.len() * 2;
        loop {
            let node = &self.arena[node_idx.as_usize()];
            if node.prefix_len as usize >= max_nib {
                if node.is_terminal() {
                    if self.key_matches(node.leaf, key) {
                        return Some(node.leaf.as_usize());
                    }
                }
                return None;
            }
            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[nib];
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if node.is_leaf(nib) {
                let key_index = slot;
                return if self.key_matches(key_index, key) {
                    Some(key_index.as_usize())
                } else {
                    None
                };
            }
            node_idx = slot;
        }
    }

    /// Unchecked lookup — assumes the key is present in the trie.
    ///
    /// # Safety
    ///
    /// The key **must** have been inserted into this trie. If the key is not
    /// present, the result is unspecified.
    pub unsafe fn get_unchecked(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: PTR = PTR::zero();
        let max_nib = key.len() * 2;
        loop {
            let node = unsafe { self.arena.get_unchecked(node_idx.as_usize()) };
            let prefix_len = node.prefix_len as usize;
            if prefix_len >= max_nib {
                debug_assert!(node.is_terminal(), "get_unchecked: key not in set");
                return Some(node.leaf.as_usize());
            }
            let nib = key_nibble_at(key, prefix_len) as usize;
            let slot = unsafe { *node.children.get_unchecked(nib) };
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if node.is_leaf(nib) {
                return Some(slot.as_usize());
            }
            node_idx = slot;
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|ki| &self.values[ki])
    }

    // -------------------------------------------------------------------
    // Insertion
    // -------------------------------------------------------------------

    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        if key.len() > self.max_len {
            return Err(());
        }
        // Overflow: arena/key indices must fit in PTR. max_value() is sentinel.
        if self.arena.len() >= PTR::max_value() {
            return Err(());
        }
        if self.values.len() + 1 >= PTR::max_value() {
            return Err(());
        }

        // Allocate slot
        let ki = self.values.len();
        let start = self.buf.len();
        self.buf.resize(start + self.max_len, 0);
        self.buf[start..start + key.len()].copy_from_slice(&key);
        self.values.push(value);
        self.lens.push(key.len() as u16);
        let new_ki = PTR::from_usize(ki);
        let max_nib = key.len() * 2;

        if self.arena.is_empty() {
            if max_nib == 0 {
                // Empty key — root is terminal
                let mut root = FixedLenNode::new();
                root.set_terminal(true);
                root.leaf = new_ki;
                self.arena.push(root);
                return Ok(new_ki.as_usize());
            }
            let first_nib = key_nibble_at(&key, 0) as usize;
            let mut root = FixedLenNode::new();
            root.set_leaf_child(first_nib, new_ki);
            root.leaf = new_ki;
            self.arena.push(root);
            return Ok(new_ki.as_usize());
        }

        let mut node_idx: PTR = PTR::zero();
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx.as_usize()];
            let ref_key = self.key_slice(node.leaf);
            let prefix_len = node.prefix_len as usize;

            match simd_find_divergence::<8>(&key, ref_key, confirmed) {
                DivergeResult::Duplicate => {
                    // Roll back allocation
                    self.buf.truncate(start);
                    self.values.pop();
                    self.lens.pop();
                    return Err(());
                }
                DivergeResult::At(diverge) if diverge < prefix_len => {
                    // Divergence before discriminating nibble — split this node
                    let new_nib = key_nibble_at(&key, diverge) as usize;
                    let ref_nib = key_nibble_at(ref_key, diverge) as usize;

                    let mut new_parent = FixedLenNode::new();
                    new_parent.prefix_len = diverge as u16;

                    if diverge >= max_nib {
                        // New key ends at the split point — terminal
                        new_parent.set_terminal(true);
                        new_parent.leaf = new_ki;
                    } else {
                        new_parent.set_leaf_child(new_nib, new_ki);
                        new_parent.leaf = new_ki;
                    }

                    let old_node = std::mem::replace(
                        &mut self.arena[node_idx.as_usize()],
                        new_parent,
                    );
                    let old_idx = PTR::from_usize(self.arena.len());
                    self.arena.push(old_node);

                    self.arena[node_idx.as_usize()].set_internal_child(ref_nib, old_idx);
                    self.sort_internal_children(node_idx);

                    return Ok(new_ki.as_usize());
                }
                DivergeResult::At(_) => {
                    // Divergence at or after prefix_len — follow the child.
                    if max_nib <= prefix_len {
                        // Key exhausted at this node — mark terminal
                        self.arena[node_idx.as_usize()].set_terminal(true);
                        self.arena[node_idx.as_usize()].leaf = new_ki;
                        return Ok(new_ki.as_usize());
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(&key, prefix_len) as usize;
                    let slot = node.children[nib];

                    if slot == PTR::max_value_sentinel() {
                        // Empty slot — new key diverges here
                        self.arena[node_idx.as_usize()].set_leaf_child(nib, new_ki);
                        return Ok(new_ki.as_usize());
                    }

                    if node.is_leaf(nib) {
                        let existing_ki = slot;
                        let existing_key = self.key_slice(existing_ki);

                        match simd_find_divergence::<8>(&key, existing_key, confirmed) {
                            DivergeResult::Duplicate => {
                                // Roll back
                                self.buf.truncate(start);
                                self.values.pop();
                                self.lens.pop();
                                return Err(());
                            }
                            DivergeResult::At(d) => {
                                let mut split_node = FixedLenNode::new();
                                split_node.prefix_len = d as u16;

                                if d >= max_nib {
                                    // New key ends at split — terminal
                                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                                    split_node.set_terminal(true);
                                    split_node.leaf = new_ki;
                                    split_node.set_leaf_child(exist_nib, existing_ki);
                                } else if d >= existing_key.len() * 2 {
                                    // Existing key ends at split — terminal
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    split_node.set_terminal(true);
                                    split_node.leaf = existing_ki;
                                    split_node.set_leaf_child(new_nib, new_ki);
                                } else {
                                    // Neither key ends — they diverge
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                                    debug_assert_ne!(new_nib, exist_nib);
                                    split_node.set_leaf_child(new_nib, new_ki);
                                    split_node.set_leaf_child(exist_nib, existing_ki);
                                    split_node.leaf = existing_ki;
                                }

                                let split_idx = PTR::from_usize(self.arena.len());
                                self.arena.push(split_node);
                                self.arena[node_idx.as_usize()].set_internal_child(nib, split_idx);
                                self.sort_internal_children(node_idx);

                                return Ok(new_ki.as_usize());
                            }
                        }
                    }

                    // Internal node — descend
                    node_idx = slot;
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Optimization (DFS key-sorted buf + values rewrite)
    // -------------------------------------------------------------------

    /// Rewrite `buf` and `values` so that keys appear in sorted order, with
    /// contiguous layout for sequential access during iteration.
    ///
    /// After `optimize()`, a forward iteration visits keys in ascending memory
    /// order within `buf`. Leaf indices in arena nodes are remapped to reflect
    /// the new positions.
    ///
    /// No-op for empty tries.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let n = self.values.len();
        let mut new_buf = vec![0u8; n * self.max_len];
        let mut new_values = Vec::with_capacity(n);
        let mut new_lens = Vec::with_capacity(n);
        // Safety: we'll fill all n slots, so this is safe
        unsafe { new_values.set_len(n); }
        unsafe { new_lens.set_len(n); }

        let mut cursor: usize = 0;
        let mut remap: Vec<usize> = vec![0; n]; // old ki -> new ki

        self.walk_optimize(PTR::zero(), &mut new_buf, &mut new_values, &mut new_lens, &mut remap, &mut cursor);

        // Remap all leaf indices in arena nodes
        for node in &mut self.arena {
            if node.leaf != PTR::max_value_sentinel() {
                node.leaf = PTR::from_usize(remap[node.leaf.as_usize()]);
            }
            for nib in 0..16 {
                if node.is_leaf(nib) && node.children[nib] != PTR::max_value_sentinel() {
                    node.children[nib] = PTR::from_usize(remap[node.children[nib].as_usize()]);
                }
            }
        }

        self.buf = new_buf;
        self.values = new_values;
        self.lens = new_lens;
    }

    fn walk_optimize(
        &mut self,
        node_idx: PTR,
        new_buf: &mut [u8],
        new_values: &mut [T],
        new_lens: &mut [u16],
        remap: &mut [usize],
        cursor: &mut usize,
    ) {
        let node = self.arena[node_idx.as_usize()]; // copy to avoid borrow conflicts

        if node.is_terminal() {
            let old_ki = node.leaf.as_usize();
            let new_ki = *cursor;
            let old_start = old_ki * self.max_len;
            let new_start = new_ki * self.max_len;
            new_buf[new_start..new_start + self.max_len]
                .copy_from_slice(&self.buf[old_start..old_start + self.max_len]);
            // SAFETY: we're writing into a set_len'd Vec; all slots will be written
            unsafe {
                std::ptr::write(new_values.as_mut_ptr().add(new_ki), std::ptr::read(self.values.as_ptr().add(old_ki)));
            }
            new_lens[new_ki] = self.lens[old_ki];
            remap[old_ki] = new_ki;
            *cursor += 1;
        }

        for nib in 0..16 {
            if node.children[nib] == PTR::max_value_sentinel() {
                continue;
            }
            if node.is_leaf(nib) {
                let old_ki = node.children[nib].as_usize();
                let new_ki = *cursor;
                let old_start = old_ki * self.max_len;
                let new_start = new_ki * self.max_len;
                new_buf[new_start..new_start + self.max_len]
                    .copy_from_slice(&self.buf[old_start..old_start + self.max_len]);
                unsafe {
                    std::ptr::write(new_values.as_mut_ptr().add(new_ki), std::ptr::read(self.values.as_ptr().add(old_ki)));
                }
                new_lens[new_ki] = self.lens[old_ki];
                remap[old_ki] = new_ki;
                *cursor += 1;
            } else {
                self.walk_optimize(node.children[nib], new_buf, new_values, new_lens, remap, cursor);
            }
        }
    }

    /// Optimize after insert if `values.len()` is a power of two.
    fn maybe_optimize(&mut self) {
        let n = self.values.len();
        if n > 0 && n.is_power_of_two() {
            self.optimize();
        }
    }

    /// Insert and auto-optimize on power-of-two sizes.
    pub fn insert_auto(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        let result = self.insert(key, value)?;
        self.maybe_optimize();
        Ok(result)
    }

    // -------------------------------------------------------------------
    // Iteration
    // -------------------------------------------------------------------

    pub fn iter(&self) -> FixedLenIter<'_, T, PTR> {
        FixedLenIter::new(self)
    }

    pub fn iter_last(&self) -> FixedLenIter<'_, T, PTR> {
        FixedLenIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<Vec<u8>>, Vec<T>) {
        let keys: Vec<Vec<u8>> = (0..self.values.len())
            .map(|i| self.key_slice(PTR::from_usize(i)).to_vec())
            .collect();
        (keys, self.values)
    }

    // -------------------------------------------------------------------
    // Arena maintenance
    // -------------------------------------------------------------------

    fn swap_arena(&mut self, a: PTR, b: PTR) {
        if a == b {
            return;
        }
        self.arena.swap(a.as_usize(), b.as_usize());
        for node in &mut self.arena {
            for nib in 0..16 {
                let child = node.children[nib];
                if child != PTR::max_value_sentinel() && !node.is_leaf(nib) {
                    if child == a {
                        node.children[nib] = b;
                    } else if child == b {
                        node.children[nib] = a;
                    }
                }
            }
        }
    }

    fn sort_internal_children(&mut self, node_idx: PTR) {
        let mut internals: [u8; 16] = [0; 16];
        let mut arena_ids: [PTR; 16] = [PTR::max_value_sentinel(); 16];
        let mut count = 0usize;
        {
            let node = &self.arena[node_idx.as_usize()];
            for nib in 0u8..16 {
                if node.children[nib as usize] != PTR::max_value_sentinel()
                    && !node.is_leaf(nib as usize)
                {
                    internals[count] = nib;
                    arena_ids[count] = node.children[nib as usize];
                    count += 1;
                }
            }
        }
        if count <= 1 {
            return;
        }
        let max_arena_idx = (0..count).fold(PTR::zero(), |m, i| {
            if arena_ids[i].as_usize() > m.as_usize() { arena_ids[i] } else { m }
        });
        let insert_pos = (0..count).find(|&i| arena_ids[i] == max_arena_idx).unwrap();
        for i in insert_pos..count - 1 {
            self.swap_arena(arena_ids[i], arena_ids[i + 1]);
            let tmp = arena_ids[i];
            arena_ids[i] = arena_ids[i + 1];
            arena_ids[i + 1] = tmp;
        }
    }

    // -------------------------------------------------------------------
    // Capacity
    // -------------------------------------------------------------------

    pub fn near_capacity(&self) -> bool {
        self.arena.len() >= PTR::max_value() || self.values.len() + 1 >= PTR::max_value()
    }
}

impl<T, PTR: TrieIndex> Default for FixedLenNibbleTrie<T, PTR> {
    fn default() -> Self {
        // Default max_len of 256 — reasonable for most string keys.
        // Users should call new() with an appropriate max_len.
        Self::new(256)
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

const TERMINAL_NIB: usize = 16;

pub struct FixedLenIter<'a, T, PTR: TrieIndex> {
    trie: &'a FixedLenNibbleTrie<T, PTR>,
    stack: Vec<(PTR, u16, usize)>,
}

impl<'a, T, PTR: TrieIndex> FixedLenIter<'a, T, PTR> {
    fn new(trie: &'a FixedLenNibbleTrie<T, PTR>) -> Self {
        if trie.arena.is_empty() {
            return FixedLenIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].children_mask();
        let nib = if trie.arena[0].is_terminal() { TERMINAL_NIB } else { usize::MAX };
        FixedLenIter { trie, stack: vec![(PTR::zero(), mask, nib)] }
    }

    fn new_last(trie: &'a FixedLenNibbleTrie<T, PTR>) -> Self {
        if trie.arena.is_empty() {
            return FixedLenIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut idx: PTR = PTR::zero();
        loop {
            let node = &trie.arena[idx.as_usize()];
            let mask = node.children_mask();
            if mask != 0 {
                let nib = 15 - mask.leading_zeros() as usize;
                stack.push((idx, mask, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    idx = node.children[nib];
                }
            } else if node.is_terminal() {
                stack.push((idx, mask, TERMINAL_NIB));
                break;
            } else {
                break;
            }
        }
        FixedLenIter { trie, stack }
    }

    #[inline]
    fn descend_first(&mut self, mut idx: PTR) {
        loop {
            let node = &self.trie.arena[idx.as_usize()];
            if node.is_terminal() {
                let mask = node.children_mask();
                self.stack.push((idx, mask, TERMINAL_NIB));
                return;
            }
            let mask = node.children_mask();
            debug_assert!(mask != 0, "descend_first: non-terminal node with no children");
            let nib = mask.trailing_zeros() as usize;
            self.stack.push((idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                idx = node.children[nib];
            }
        }
    }

    #[inline]
    fn descend_last(&mut self, mut idx: PTR) {
        loop {
            let node = &self.trie.arena[idx.as_usize()];
            if node.is_terminal() {
                let mask = node.children_mask();
                if mask == 0 {
                    self.stack.push((idx, mask, TERMINAL_NIB));
                    return;
                }
            }
            let mask = node.children_mask();
            if mask == 0 {
                if node.is_terminal() {
                    self.stack.push((idx, mask, TERMINAL_NIB));
                }
                return;
            }
            let nib = 15 - mask.leading_zeros() as usize;
            self.stack.push((idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                idx = node.children[nib];
            }
        }
    }

    #[inline]
    fn push_next_child(&mut self, arena_idx: PTR, mask: u16, start_nib: usize) -> bool {
        let shifted = if start_nib >= 16 { 0u16 } else { mask >> start_nib };
        if shifted == 0 {
            return false;
        }
        let nib = start_nib + shifted.trailing_zeros() as usize;
        debug_assert!(nib < 16);
        debug_assert!(mask & (1 << nib) != 0);
        self.stack.push((arena_idx, mask, nib));
        if !self.trie.arena[arena_idx.as_usize()].is_leaf(nib) {
            self.descend_first(self.trie.arena[arena_idx.as_usize()].children[nib]);
        }
        true
    }

    #[inline]
    fn backtrack_to_next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (parent_idx, parent_mask, child_nib) = self.stack.pop()?;
            if self.push_next_child(parent_idx, parent_mask, child_nib + 1) {
                return self.current();
            }
        }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let &(arena_idx, _mask, nib) = self.stack.last()?;
        if nib == usize::MAX {
            return None;
        }
        let node = &self.trie.arena[arena_idx.as_usize()];
        if nib == TERMINAL_NIB {
            let key = self.trie.key_slice(node.leaf);
            let value = &self.trie.values[node.leaf.as_usize()];
            Some((key, value))
        } else if let Some(key_index) = node.leaf_key_index(nib) {
            let key = self.trie.key_slice(key_index);
            let value = &self.trie.values[key_index.as_usize()];
            Some((key, value))
        } else {
            None
        }
    }

    pub fn current_index(&self) -> Option<usize> {
        let &(arena_idx, _mask, nib) = self.stack.last()?;
        if nib == usize::MAX {
            return None;
        }
        let node = &self.trie.arena[arena_idx.as_usize()];
        if nib == TERMINAL_NIB {
            Some(node.leaf.as_usize())
        } else {
            node.leaf_key_index(nib).map(|ki| ki.as_usize())
        }
    }

    #[inline]
    fn advance_next(&mut self) -> bool {
        loop {
            let (arena_idx, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                if self.push_next_child(arena_idx, mask, 0) {
                    return true;
                }
                continue;
            }

            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };
            if self.push_next_child(arena_idx, mask, search_start) {
                return true;
            }
        }
    }

    #[inline]
    fn advance_prev(&mut self) -> bool {
        loop {
            let (arena_idx, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                continue;
            }

            if nib == 0 || nib == usize::MAX {
                if self.trie.arena[arena_idx.as_usize()].is_terminal() {
                    self.stack.push((arena_idx, mask, TERMINAL_NIB));
                    return true;
                }
                continue;
            }

            let mask_below = mask & ((1 << nib) - 1);
            if mask_below != 0 {
                let prev_nib = 15 - mask_below.leading_zeros() as usize;
                self.stack.push((arena_idx, mask, prev_nib));
                if !self.trie.arena[arena_idx.as_usize()].is_leaf(prev_nib) {
                    self.descend_last(self.trie.arena[arena_idx.as_usize()].children[prev_nib]);
                }
                return true;
            }

            if self.trie.arena[arena_idx.as_usize()].is_terminal() {
                self.stack.push((arena_idx, mask, TERMINAL_NIB));
                return true;
            }
        }
    }

    #[inline]
    pub fn next_index(&mut self) -> Option<usize> {
        if self.advance_next() { self.current_index() } else { None }
    }

    #[inline]
    pub fn prev_index(&mut self) -> Option<usize> {
        if self.advance_prev() { self.current_index() } else { None }
    }

    #[inline]
    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        if self.advance_next() { self.current() } else { None }
    }

    #[inline]
    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        if self.advance_prev() { self.current() } else { None }
    }

    pub fn seek(&mut self, key: &[u8]) -> Option<(&[u8], &T)> {
        if self.trie.arena.is_empty() {
            self.stack.clear();
            return None;
        }

        self.stack.clear();
        let mut node_idx: PTR = PTR::zero();
        let max_nib = key.len() * 2;

        loop {
            let node = &self.trie.arena[node_idx.as_usize()];
            let mask = node.children_mask();

            if node.is_terminal() && node.prefix_len as usize >= max_nib {
                let node_key = self.trie.key_slice(node.leaf);
                if node_key >= key {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                    return self.current();
                }
            }

            if node.prefix_len as usize >= max_nib {
                if self.push_next_child(node_idx, mask, 0) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[nib];
            if slot != PTR::max_value_sentinel() {
                self.stack.push((node_idx, mask, nib));
                if node.is_leaf(nib) {
                    let leaf_key = self.trie.key_slice(slot);
                    if leaf_key >= key {
                        return self.current();
                    }
                    return self.next();
                } else {
                    node_idx = slot;
                    continue;
                }
            }

            if self.push_next_child(node_idx, mask, nib + 1) {
                return self.current();
            }
            return self.backtrack_to_next();
        }
    }
}

// ---------------------------------------------------------------------------
// TinyTrieMap implementation
// ---------------------------------------------------------------------------

impl TinyTrieMap for FixedLenNibbleTrie<usize, u32> {
    fn trie_new() -> Self {
        Self::new(256)
    }

    fn trie_insert(&mut self, key: Vec<u8>, value: usize) {
        self.insert(key, value).unwrap();
    }

    fn trie_get(&self, key: &[u8]) -> Option<usize> {
        self.get(key)
    }

    fn trie_iter_fwd(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.next() { f(k, v); }
    }

    fn trie_iter_rev(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter_last();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.prev() { f(k, v); }
    }

    fn trie_iter_fwd_index(&self, mut f: impl FnMut(usize)) {
        let mut it = self.iter();
        if let Some(i) = it.current_index() { f(i); }
        while let Some(i) = it.next_index() { f(i); }
    }

    fn trie_iter_rev_index(&self, mut f: impl FnMut(usize)) {
        let mut it = self.iter_last();
        if let Some(i) = it.current_index() { f(i); }
        while let Some(i) = it.prev_index() { f(i); }
    }

    fn trie_len(&self) -> usize {
        self.len()
    }

    fn trie_optimize(&mut self) {
        self.optimize();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/fixed_len_nibble_trie.rs"]
mod tests;