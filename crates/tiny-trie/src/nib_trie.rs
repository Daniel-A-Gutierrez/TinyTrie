//! Nib Trie — a fixed-fanout radix trie indexed by 2-bit words (nibs).
//!
//! Each node has 4 child slots (one per 2-bit value 0–3), addressed by direct
//! indexing. This trades space for simplicity and lookup speed compared to a
//! binary trie, while using less space per node than a 16-way nibble trie.
//!
//! # No Stacking
//!
//! Unlike NibbleTrie, NibTrie does not support vnode stacking. Each physical
//! node holds exactly one logical trie node. The `occupancy` and `leaf_mask`
//! fields are `u8` (4 bits used) rather than `u16` (16 bits).
//!
//! # Terminal Nodes
//!
//! Keys that are prefixes of other keys are represented by a `terminal` flag on
//! the node where the key ends, rather than a null-byte leaf child. This
//! eliminates null terminators, allows `0x00` bytes in keys, and makes `get()`
//! accept plain `&[u8]`.
//!
//! # Key Index Encoding
//!
//! Real keys start at index 1 (index 0 is the dummy entry). The sentinel
//! `PTR::max_value()` marks empty slots in `children[]`.
//!
//! # Nib Addressing
//!
//! Each byte contains 4 nib positions:
//! - nib 0: bits 7–6 of byte 0
//! - nib 1: bits 5–4 of byte 0
//! - nib 2: bits 3–2 of byte 0
//! - nib 3: bits 1–0 of byte 0
//! - nib 4: bits 7–6 of byte 1
//! - etc.
//!
//! Total nib count for a key of length L is `L * 4`.

use tiny_trie_trait::TinyTrieMap;
use crate::nibble_trie::TrieIndex;
use std::{fmt, simd::{Simd, cmp::SimdPartialEq}};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the nib trie arena.
///
/// Each node has 4 child slots (one per 2-bit value), a leaf reference key,
/// a prefix length in nib positions, and bitmasks for leaf/occupancy tracking.
///
/// Layout with PTR=u32, LEN=u16: 24 bytes (4×u32 + u32 + u16 + 3×u8 + padding).
#[derive(Copy, Clone)]
pub struct NibNode<PTR: TrieIndex = u32, LEN: TrieIndex = u16> {
    pub children: [PTR; 4],     // one per 2-bit value; PTR::MAX = empty
    pub leaf: PTR,              // key index for prefix comparison
    pub prefix_len: LEN,        // prefix length in nibs (2-bit positions)
    pub leaf_mask: u8,          // bit N = children[N] is leaf key index
    pub occupancy: u8,          // bit N = slot N is occupied
    pub terminal: u8,           // bit 0 = this node is terminal
}

impl<PTR: TrieIndex, LEN: TrieIndex> NibNode<PTR, LEN> {
    pub fn new() -> Self {
        NibNode {
            children: [PTR::max_value_sentinel(); 4],
            leaf: PTR::max_value_sentinel(),
            prefix_len: LEN::zero(),
            leaf_mask: 0,
            occupancy: 0,
            terminal: 0,
        }
    }

    /// Check if this node is terminal (represents a key that ends here).
    #[inline]
    pub fn is_terminal(&self) -> bool {
        self.terminal != 0
    }

    /// Set or clear the terminal flag.
    #[inline]
    fn set_terminal(&mut self, val: bool) {
        if val {
            self.terminal = 1;
        } else {
            self.terminal = 0;
        }
    }

    /// Check if nib slot `nib` is a leaf (key index).
    #[inline]
    pub fn is_leaf(&self, nib: usize) -> bool {
        debug_assert!(nib < 4);
        (self.leaf_mask >> nib) & 1 == 1
    }

    /// Set the leaf flag for nib slot `nib`.
    #[inline]
    fn set_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 4);
        self.leaf_mask |= 1 << nib;
    }

    /// Clear the leaf flag for nib slot `nib`.
    #[inline]
    fn clear_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 4);
        self.leaf_mask &= !(1 << nib);
    }

    /// Check if nib slot `nib` is occupied.
    #[inline]
    pub fn is_occupied(&self, nib: usize) -> bool {
        debug_assert!(nib < 4);
        (self.occupancy >> nib) & 1 == 1
    }

    /// Set the occupancy bit for nib slot `nib`.
    #[inline]
    fn set_occupied(&mut self, nib: usize) {
        debug_assert!(nib < 4);
        self.occupancy |= 1 << nib;
    }

    /// Store a leaf key index at `nib`. Key index must not be the sentinel.
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, key_index: PTR) {
        debug_assert!(nib < 4);
        debug_assert!(key_index != PTR::max_value_sentinel(), "sentinel key index");
        self.set_leaf(nib);
        self.set_occupied(nib);
        self.children[nib] = key_index;
    }

    /// Store an arena index at `nib` (internal node reference).
    #[inline]
    fn set_internal_child(&mut self, nib: usize, addr: PTR) {
        debug_assert!(nib < 4);
        debug_assert!(addr != PTR::max_value_sentinel(), "sentinel address");
        self.clear_leaf(nib);
        self.set_occupied(nib);
        self.children[nib] = addr;
    }

    /// Decode a leaf child at `nib` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize) -> Option<PTR> {
        debug_assert!(nib < 4);
        if self.is_leaf(nib) && self.is_occupied(nib) {
            Some(self.children[nib])
        } else {
            None
        }
    }

    /// Compute a 4-bit mask where bit N is set if `children[N]` is not the sentinel.
    #[inline]
    pub fn children_mask(&self) -> u8 {
        self.occupancy
    }

    /// Promote this node's PTR type to a wider one.
    pub fn promote<NewPTR: TrieIndex>(self) -> NibNode<NewPTR, LEN> {
        let mut children = [NewPTR::max_value_sentinel(); 4];
        for i in 0..4 {
            if self.occupancy & (1 << i) != 0 {
                children[i] = NewPTR::from_usize(self.children[i].as_usize());
            }
        }
        NibNode {
            children,
            leaf: if self.leaf == PTR::max_value_sentinel() {
                NewPTR::max_value_sentinel()
            } else {
                NewPTR::from_usize(self.leaf.as_usize())
            },
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            occupancy: self.occupancy,
            terminal: self.terminal,
        }
    }

    /// Demote this node's PTR type to a narrower one.
    /// Returns `Err(self)` if any address doesn't fit.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<NibNode<NewPTR, LEN>, Self> {
        for i in 0..4 {
            if self.occupancy & (1 << i) != 0 {
                if self.children[i].as_usize() > NewPTR::max_value() {
                    return Err(self);
                }
            }
        }
        if self.leaf != PTR::max_value_sentinel() && self.leaf.as_usize() > NewPTR::max_value() {
            return Err(self);
        }
        let mut children = [NewPTR::max_value_sentinel(); 4];
        for i in 0..4 {
            if self.occupancy & (1 << i) != 0 {
                children[i] = NewPTR::from_usize(self.children[i].as_usize());
            }
        }
        Ok(NibNode {
            children,
            leaf: if self.leaf == PTR::max_value_sentinel() {
                NewPTR::max_value_sentinel()
            } else {
                NewPTR::from_usize(self.leaf.as_usize())
            },
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            occupancy: self.occupancy,
            terminal: self.terminal,
        })
    }
}

impl<PTR: TrieIndex, LEN: TrieIndex> fmt::Debug for NibNode<PTR, LEN> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active: Vec<(usize, &str, PTR)> = (0..4)
            .filter(|&n| self.occupancy & (1 << n) != 0)
            .map(|n| {
                let tag = if (self.leaf_mask >> n) & 1 == 1 { "L" } else { "I" };
                (n, tag, self.children[n])
            })
            .collect();
        f.debug_struct("NibNode")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("{:04b}", self.leaf_mask))
            .field("occupancy", &format_args!("{:04b}", self.occupancy))
            .field("terminal", &self.terminal)
            .field("leaf", &self.leaf)
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// NibTrie
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NibTrie<T, PTR: TrieIndex = u32, LEN: TrieIndex = u16> {
    pub arena: Vec<NibNode<PTR, LEN>>,
    pub buf: Vec<u8>,                // all keys concatenated (no null terminators)
    pub index: Vec<(usize, LEN)>,   // (offset into buf, len) per key — offset is usize, len is compact
    pub values: Vec<T>,              // values[i] ↔ index[i]
}

// ---------------------------------------------------------------------------
// Divergence result
// ---------------------------------------------------------------------------

enum DivergeResult {
    /// The keys are identical (same nib count, same content).
    Duplicate,
    /// The keys diverge at this nib position, or one key is a prefix of the other.
    At(usize),
}

enum PrefixCheck {
    /// The keys match at every nib position in `from..to`.
    Matches,
    /// The keys diverge at this nib position (within `from..to`).
    Diverges(usize),
}

// ---------------------------------------------------------------------------
// SIMD helpers
// ---------------------------------------------------------------------------

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
// Nib helpers
// ---------------------------------------------------------------------------

/// Extract the 2-bit nib at position `idx` from `key`.
///
/// Each byte contains 4 nibs:
/// - nib 0: bits 7–6 (most significant pair)
/// - nib 1: bits 5–4
/// - nib 2: bits 3–2
/// - nib 3: bits 1–0 (least significant pair)
///
/// Past the end of the key, returns 0 (implicit zero padding for ordering).
#[inline]
fn key_nib_at(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 4;
    if byte_idx < key.len() {
        let shift = 6 - 2 * (idx % 4); // 6, 4, 2, 0
        (key[byte_idx] >> shift) & 0x03
    } else {
        0
    }
}

/// Unchecked version of `key_nib_at`.
///
/// # Safety
/// `idx / 4` must be < `key.len()`.
#[inline]
unsafe fn key_nib_at_unchecked(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 4;
    debug_assert!(byte_idx < key.len(), "nib {idx} out of bounds for key len {}", key.len());
    let shift = 6 - 2 * (idx % 4);
    (unsafe { *key.get_unchecked(byte_idx) } >> shift) & 0x03
}

#[inline]
fn nib_count(key: &[u8]) -> usize {
    key.len() * 4
}

/// Given a non-zero XOR of two differing bytes, return the nib position of the
/// first divergence. Uses `leading_zeros` for branchless computation:
/// `group = lz / 2`, so `nib = byte_idx * 4 + lz / 2`.
#[inline]
fn diverging_nib(xor: u8, byte_idx: usize) -> usize {
    byte_idx * 4 + (xor.leading_zeros() as usize) / 2
}

/// Scan two keys from `from` onward to find the first diverging nib.
#[inline]
fn find_divergence(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult {
    let total_a = nib_count(key_a);
    let total_b = nib_count(key_b);
    let min = total_a.min(total_b);
    let mut d = from;
    while d < min {
        if key_nib_at(key_a, d) != key_nib_at(key_b, d) {
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
{
    let minlen = key_a.len().min(key_b.len());
    let mut i = from / 4; // byte containing nib `from`

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor = unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            return DivergeResult::At(diverging_nib(xor, diff_byte_idx));
        }
        i += N;
    }

    // Scalar tail
    find_divergence(key_a, key_b, i * 4)
}

/// Scan nibs `from..to` of two keys. Returns `Diverges(pos)` if they differ
/// at any nib in that range, or `Matches` if they agree throughout.
#[inline]
fn check_prefix(key_a: &[u8], key_b: &[u8], from: usize, to: usize) -> PrefixCheck {
    for nib in from..to {
        if key_nib_at(key_a, nib) != key_nib_at(key_b, nib) {
            return PrefixCheck::Diverges(nib);
        }
    }
    PrefixCheck::Matches
}

/// SIMD-accelerated bounded prefix check for 2-bit nibs.
fn simd_check_prefix<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize, to: usize) -> PrefixCheck
where
{
    if from >= to {
        return PrefixCheck::Matches;
    }

    let from_byte = from / 4;
    let to_byte = (to + 3) / 4; // first byte fully outside the nib range
    let minlen = key_a.len().min(key_b.len()).min(to_byte);
    let mut i = from_byte;

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor = unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            let nib = diverging_nib(xor, diff_byte_idx);
            if nib < to {
                return PrefixCheck::Diverges(nib);
            }
            // Divergence past the bound — keys match within range
            return PrefixCheck::Matches;
        }
        i += N;
    }

    // Scalar tail
    check_prefix(key_a, key_b, i * 4, to)
}

// ---------------------------------------------------------------------------
// NibTrie methods
// ---------------------------------------------------------------------------

impl<T, PTR: TrieIndex, LEN: TrieIndex> NibTrie<T, PTR, LEN> {
    /// Return the key slice for `key_index`.
    #[inline]
    fn key_slice(&self, key_index: PTR) -> &[u8] {
        let (off, len) = self.index[key_index.as_usize()];
        &self.buf[off..off + len.as_usize()]
    }

    pub fn new() -> Self {
        NibTrie {
            arena: Vec::new(),
            buf: vec![0],           // buf[0] = dummy (unused byte)
            index: vec![(0, LEN::zero())],   // index[0] = dummy entry
            values: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.index.len() - 1 // subtract dummy
    }

    pub fn is_empty(&self) -> bool {
        self.index.len() == 1 // only the dummy
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: usize = 0;
        let max_nib = key.len() * 4;
        loop {
            let node = &self.arena[node_idx];
            let prefix_len = node.prefix_len.as_usize();

            // Key nibs exhausted — check if this node is terminal.
            if prefix_len >= max_nib {
                if node.is_terminal() {
                    let ki = node.leaf;
                    let (off, len) = self.index[ki.as_usize()];
                    let key_in_buf = &self.buf[off..off + len.as_usize()];
                    if key.len() == len.as_usize() && simd_eq(&key_in_buf[..key.len()], key) {
                        return Some(ki.as_usize());
                    }
                }
                return None;
            }

            // Safe to use unchecked: prefix_len < max_nib guarantees byte_idx < key.len()
            let nib = unsafe { key_nib_at_unchecked(key, prefix_len) } as usize;
            let slot = node.children[nib];
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if (node.leaf_mask >> nib) & 1 == 1 {
                // Leaf — verify full key match
                let key_index = slot;
                return if simd_eq(self.key_slice(key_index), key) {
                    Some(key_index.as_usize())
                } else {
                    None
                };
            }
            // Internal child — descend
            node_idx = slot.as_usize();
        }
    }

    /// Unchecked lookup — assumes the key is present in the trie.
    ///
    /// # Safety
    /// The key **must** have been inserted into this trie.
    pub unsafe fn get_unchecked(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: usize = 0;
        let max_nib = key.len() * 4;
        loop {
            let node = unsafe { self.arena.get_unchecked(node_idx) };
            let prefix_len = node.prefix_len.as_usize();
            if prefix_len >= max_nib {
                debug_assert!(node.is_terminal(), "get_unchecked: key not in set");
                return Some(node.leaf.as_usize());
            }
            let nib = unsafe { key_nib_at_unchecked(key, prefix_len) } as usize;
            let slot = unsafe { *node.children.get_unchecked(nib) };
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if (node.leaf_mask >> nib) & 1 == 1 {
                return Some(slot.as_usize());
            }
            node_idx = slot.as_usize();
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1])
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> NibIter<'_, T, PTR, LEN> {
        NibIter::new(self)
    }

    pub fn iter_last(&self) -> NibIter<'_, T, PTR, LEN> {
        NibIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<Vec<u8>>, Vec<T>) {
        let buf = self.buf;
        let keys: Vec<Vec<u8>> = self.index.into_iter().skip(1).map(|(off, len)| {
            buf[off..off + len.as_usize()].to_vec()
        }).collect();
        (keys, self.values)
    }

    // -----------------------------------------------------------------------
    // Capacity
    // -----------------------------------------------------------------------

    pub fn near_capacity(&self) -> bool {
        self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value()
    }

    // -----------------------------------------------------------------------
    // Optimize (DFS key-sorted buf rewrite)
    // -----------------------------------------------------------------------

    /// Rewrite `buf` in DFS order for cache locality.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let mut new_buf = vec![0u8; self.buf.len()];
        let mut cursor: usize = 1; // position 0 is the dummy byte

        // Remap table: maps old arena index → new arena index.
        let mut remap: Vec<usize> = vec![0; self.arena.len()];

        let mut new_arena: Vec<NibNode<PTR, LEN>> = Vec::new();

        // Collect key indices in DFS visitation order for index/values sorting
        let mut dfs_key_order: Vec<PTR> = Vec::new();

        self.walk_optimize(
            0,
            &mut new_buf, &mut cursor,
            &mut remap, &mut new_arena,
            &mut dfs_key_order,
        );

        new_buf.truncate(cursor);
        self.buf = new_buf;
        self.arena = new_arena;

        // Remap all internal child addresses in the new arena
        for node in &mut self.arena {
            for nib in 0..4 {
                if node.occupancy & (1 << nib) != 0 && !node.is_leaf(nib) {
                    let old_addr = node.children[nib].as_usize();
                    debug_assert!(old_addr < remap.len(), "old_addr {} >= remap.len() {}", old_addr, remap.len());
                    debug_assert!(!(remap[old_addr] == 0 && old_addr != 0), "remap[{}] == 0 but old_addr != 0", old_addr);
                    node.children[nib] = PTR::from_usize(remap[old_addr]);
                }
            }
        }

        // --- Sort index and values into DFS order ---

        let num_keys = dfs_key_order.len();
        let mut key_remap: Vec<usize> = vec![0; self.index.len()];
        key_remap[0] = 0; // dummy stays at 0
        for (new_ki, &old_ki) in dfs_key_order.iter().enumerate() {
            key_remap[old_ki.as_usize()] = new_ki + 1; // 1-based
        }

        // Remap all key index references in the arena
        for node in &mut self.arena {
            for nib in 0..4 {
                if node.occupancy & (1 << nib) != 0 && node.is_leaf(nib) {
                    let old_ki = node.children[nib].as_usize();
                    let new_ki = key_remap[old_ki];
                    node.children[nib] = PTR::from_usize(new_ki);
                }
            }
            // Remap leaf pointer (skip sentinel)
            let old_leaf = node.leaf;
            if old_leaf != PTR::max_value_sentinel() {
                let new_ki = key_remap[old_leaf.as_usize()];
                node.leaf = PTR::from_usize(new_ki);
            }
        }

        // Rebuild index in DFS order
        let mut new_index: Vec<(usize, LEN)> = vec![(0, LEN::zero()); num_keys + 1];
        new_index[0] = self.index[0]; // keep dummy entry
        for (new_ki, &old_ki) in dfs_key_order.iter().enumerate() {
            new_index[new_ki + 1] = self.index[old_ki.as_usize()];
        }

        // Reorder values to match new key ordering
        let mut new_values = Vec::with_capacity(num_keys);
        unsafe {
            let old_values_ptr = self.values.as_ptr();
            for &old_ki in &dfs_key_order {
                let old_val = std::ptr::read(old_values_ptr.add(old_ki.as_usize() - 1));
                new_values.push(old_val);
            }
        }
        unsafe { self.values.set_len(0); }
        std::mem::swap(&mut self.values, &mut new_values);
        self.index = new_index;
    }

    fn walk_optimize(
        &mut self,
        old_idx: usize,
        new_buf: &mut [u8],
        cursor: &mut usize,
        remap: &mut Vec<usize>,
        new_arena: &mut Vec<NibNode<PTR, LEN>>,
        dfs_key_order: &mut Vec<PTR>,
    ) {
        let node = self.arena[old_idx]; // copy to avoid borrow conflicts
        let occ = node.occupancy;
        let is_term = node.is_terminal();

        let new_idx = new_arena.len();
        new_arena.push(NibNode::new());
        remap[old_idx] = new_idx;

        // Populate new node fields
        new_arena[new_idx].prefix_len = node.prefix_len;
        new_arena[new_idx].occupancy = occ;
        new_arena[new_idx].leaf_mask = node.leaf_mask;
        if is_term {
            new_arena[new_idx].set_terminal(true);
        }

        // Copy key data for terminal node
        if is_term {
            let ki = node.leaf;
            let (old_off, len) = self.index[ki.as_usize()];
            let start = *cursor;
            new_buf[start..start + len.as_usize()].copy_from_slice(
                &self.buf[old_off..old_off + len.as_usize()]
            );
            self.index[ki.as_usize()].0 = *cursor;
            *cursor += len.as_usize();
            new_arena[new_idx].leaf = ki;
            dfs_key_order.push(ki);
        }

        // Recurse into children
        for nib in 0..4 {
            if (occ >> nib) & 1 == 0 {
                continue;
            }
            if node.is_leaf(nib) {
                // Leaf child — copy key data
                let ki = node.children[nib];
                let (old_off, len) = self.index[ki.as_usize()];
                let start = *cursor;
                new_buf[start..start + len.as_usize()].copy_from_slice(
                    &self.buf[old_off..old_off + len.as_usize()]
                );
                self.index[ki.as_usize()].0 = *cursor;
                *cursor += len.as_usize();
                new_arena[new_idx].children[nib] = ki;
                dfs_key_order.push(ki);
            } else {
                // Internal child — recurse, then store old address for remapping
                let child_old_addr = node.children[nib].as_usize();
                self.walk_optimize(
                    child_old_addr,
                    new_buf, cursor,
                    remap, new_arena,
                    dfs_key_order,
                );
                // Store old address so the remap loop can find it
                new_arena[new_idx].children[nib] = node.children[nib];
            }
        }

        // Propagate leaf for non-terminal nodes
        if !is_term && new_arena[new_idx].leaf == PTR::max_value_sentinel() {
            let first_nib = occ.trailing_zeros() as usize;
            if new_arena[new_idx].is_leaf(first_nib) {
                new_arena[new_idx].leaf = new_arena[new_idx].children[first_nib];
            } else {
                let child_old_addr = node.children[first_nib].as_usize();
                if child_old_addr < remap.len() {
                    let child_new_idx = remap[child_old_addr];
                    new_arena[new_idx].leaf = new_arena[child_new_idx].leaf;
                }
            }
        }
    }
}

impl<T, PTR: TrieIndex, LEN: TrieIndex> Default for NibTrie<T, PTR, LEN> {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

impl<T, PTR: TrieIndex, LEN: TrieIndex> NibTrie<T, PTR, LEN> {
    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        // Overflow checks
        if self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value() {
            return Err(());
        }
        if key.len() * 4 > LEN::max_value() {
            return Err(());
        }

        let new_index = PTR::from_usize(self.index.len());
        let key_len = LEN::from_usize(key.len());
        let offset = self.buf.len() as usize;
        self.buf.extend_from_slice(&key);
        self.index.push((offset, key_len));
        self.values.push(value);

        let max_nib = key.len() * 4;

        if self.arena.is_empty() {
            return Ok(self.insert_into_empty_trie(&key, new_index, max_nib));
        }

        let mut node_idx: usize = 0;
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx];
            let ki = node.leaf;
            let (off, ref_len) = self.index[ki.as_usize()];
            let ref_key = &self.buf[off..off + ref_len.as_usize()];
            let prefix_len = node.prefix_len.as_usize();

            match simd_check_prefix::<8>(&key, ref_key, confirmed, prefix_len) {
                PrefixCheck::Diverges(diverge) => {
                    return Ok(self.split_node_before_prefix(
                        node_idx, diverge, new_index, &key, max_nib,
                    ));
                }
                PrefixCheck::Matches => {
                    if max_nib == prefix_len {
                        if key.len() == ref_key.len() {
                            self.rollback_last_insert();
                            return Err(());
                        }
                        self.arena[node_idx].set_terminal(true);
                        self.arena[node_idx].leaf = new_index;
                        return Ok(new_index.as_usize());
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nib_at(&key, prefix_len) as usize;
                    if !node.is_occupied(nib) {
                        // Empty slot — new key diverges here
                        self.arena[node_idx].set_leaf_child(nib, new_index);
                        return Ok(new_index.as_usize());
                    }
                    let slot = node.children[nib];

                    if node.is_leaf(nib) {
                        return self.split_leaf_child(
                            nib, node_idx, slot, new_index, &key, max_nib, confirmed,
                        );
                    }

                    // Internal child — descend
                    node_idx = slot.as_usize();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Insert helpers
    // -----------------------------------------------------------------------

    #[inline]
    fn rollback_last_insert(&mut self) {
        let (off, _len) = self.index.pop().unwrap();
        self.buf.truncate(off);
        let _ = self.values.pop();
    }

    #[inline]
    fn insert_into_empty_trie(&mut self, key: &[u8], new_index: PTR, max_nib: usize) -> usize {
        if max_nib == 0 {
            let mut root = NibNode::new();
            root.set_terminal(true);
            root.leaf = new_index;
            root.prefix_len = LEN::zero();
            self.arena.push(root);
            return new_index.as_usize();
        }
        let first_nib = key_nib_at(key, 0) as usize;
        let mut root = NibNode::new();
        root.set_leaf_child(first_nib, new_index);
        root.leaf = new_index;
        root.prefix_len = LEN::zero();
        self.arena.push(root);
        new_index.as_usize()
    }

    #[inline]
    fn split_node_before_prefix(
        &mut self,
        node_idx: usize,
        diverge: usize,
        new_index: PTR,
        key: &[u8],
        max_nib: usize,
    ) -> usize {
        let node = &self.arena[node_idx];
        let ki = node.leaf;
        let (off, ref_len) = self.index[ki.as_usize()];
        let ref_key = &self.buf[off..off + ref_len.as_usize()];

        let new_nib = key_nib_at(key, diverge) as usize;
        let ref_nib = key_nib_at(ref_key, diverge) as usize;

        let mut new_parent = NibNode::new();
        new_parent.prefix_len = LEN::from_usize(diverge);

        if diverge >= max_nib {
            new_parent.set_terminal(true);
            new_parent.leaf = new_index;
        } else {
            new_parent.set_leaf_child(new_nib, new_index);
            new_parent.leaf = new_index;
        }

        let old_node = std::mem::replace(&mut self.arena[node_idx], new_parent);
        let old_addr = PTR::from_usize(self.arena.len()); // new node at next slot
        self.arena.push(old_node);

        self.arena[node_idx].set_internal_child(ref_nib, old_addr);

        new_index.as_usize()
    }

    #[inline]
    fn split_leaf_child(
        &mut self,
        nib: usize,
        node_idx: usize,
        existing_key_index: PTR,
        new_index: PTR,
        key: &[u8],
        max_nib: usize,
        confirmed: usize,
    ) -> Result<usize, ()> {
        let (existing_offset, existing_len) = self.index[existing_key_index.as_usize()];
        let existing_key = &self.buf[existing_offset..existing_offset + existing_len.as_usize()];

        match simd_find_divergence::<8>(key, existing_key, confirmed) {
            DivergeResult::Duplicate => {
                self.rollback_last_insert();
                Err(())
            }
            DivergeResult::At(d) => {
                let mut split_node = NibNode::new();
                split_node.prefix_len = LEN::from_usize(d);

                if d >= max_nib {
                    // New key ends at the split point — terminal
                    let exist_nib = key_nib_at(existing_key, d) as usize;
                    split_node.set_terminal(true);
                    split_node.leaf = new_index;
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                } else if d >= existing_key.len() * 4 {
                    // Existing key ends at the split point — terminal
                    let new_nib = key_nib_at(key, d) as usize;
                    split_node.set_terminal(true);
                    split_node.leaf = existing_key_index;
                    split_node.set_leaf_child(new_nib, new_index);
                } else {
                    // Neither key ends at the split point
                    let new_nib = key_nib_at(key, d) as usize;
                    let exist_nib = key_nib_at(existing_key, d) as usize;
                    debug_assert_ne!(new_nib, exist_nib);
                    split_node.set_leaf_child(new_nib, new_index);
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                    split_node.leaf = existing_key_index;
                }

                let split_addr = PTR::from_usize(self.arena.len());
                self.arena.push(split_node);
                self.arena[node_idx].set_internal_child(nib, split_addr);

                Ok(new_index.as_usize())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PTR width conversions (promote/demote)
// ---------------------------------------------------------------------------

impl<T, PTR: TrieIndex, LEN: TrieIndex> NibTrie<T, PTR, LEN> {
    /// Promote the arena index type to a wider PTR.
    pub fn promote<NewPTR: TrieIndex>(self) -> NibTrie<T, NewPTR, LEN> {
        let arena = self.arena.into_iter().map(|node| node.promote()).collect();
        NibTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
        }
    }

    /// Demote the arena index type to a narrower PTR.
    /// Returns `Err(self)` if any address doesn't fit.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<NibTrie<T, NewPTR, LEN>, Self> {
        if self.arena.len() > NewPTR::max_value() || self.index.len() > NewPTR::max_value() {
            return Err(self);
        }
        for node in &self.arena {
            if let Err(_) = node.demote::<NewPTR>() {
                return Err(self);
            }
        }
        let arena = self.arena.into_iter().map(|node| {
            node.demote().expect("demote capacity check should have caught this")
        }).collect();
        Ok(NibTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
        })
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// Sentinel nib value meaning "positioned at the terminal value of this node."
const TERMINAL_NIB: usize = 4;

pub struct NibIter<'a, T, PTR: TrieIndex, LEN: TrieIndex> {
    trie: &'a NibTrie<T, PTR, LEN>,
    /// Stack of (node_index, occupancy_mask, nib_position) tuples.
    stack: Vec<(usize, u8, usize)>,
}

impl<'a, T, PTR: TrieIndex, LEN: TrieIndex> NibIter<'a, T, PTR, LEN> {
    fn new(trie: &'a NibTrie<T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].occupancy;
        let nib = if trie.arena[0].is_terminal() { TERMINAL_NIB } else { usize::MAX };
        NibIter { trie, stack: vec![(0, mask, nib)] }
    }

    fn new_last(trie: &'a NibTrie<T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut node_idx: usize = 0;
        loop {
            let node = &trie.arena[node_idx];
            let mask = node.occupancy;
            if mask != 0 {
                let nib = (mask as u32).ilog2() as usize; // highest set bit (only bits 0-3 used)
                stack.push((node_idx, mask, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    node_idx = node.children[nib].as_usize();
                }
            } else if node.is_terminal() {
                stack.push((node_idx, mask, TERMINAL_NIB));
                break;
            } else {
                break;
            }
        }
        NibIter { trie, stack }
    }

    fn descend_first(&mut self, mut node_idx: usize) {
        loop {
            let node = &self.trie.arena[node_idx];
            if node.is_terminal() {
                let mask = node.occupancy;
                self.stack.push((node_idx, mask, TERMINAL_NIB));
                return;
            }
            let mask = node.occupancy;
            debug_assert!(mask != 0, "descend_first: non-terminal node with no children");
            let nib = mask.trailing_zeros() as usize;
            debug_assert!(nib < 4);
            self.stack.push((node_idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                node_idx = node.children[nib].as_usize();
            }
        }
    }

    fn descend_last(&mut self, mut node_idx: usize) {
        loop {
            let node = &self.trie.arena[node_idx];
            if node.is_terminal() && node.occupancy == 0 {
                self.stack.push((node_idx, node.occupancy, TERMINAL_NIB));
                return;
            }
            let mask = node.occupancy;
            if mask == 0 {
                if node.is_terminal() {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                }
                return;
            }
            let nib = (mask as u32).ilog2() as usize;
            debug_assert!(nib < 4);
            self.stack.push((node_idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                node_idx = node.children[nib].as_usize();
            }
        }
    }

    #[inline]
    fn push_next_child(&mut self, node_idx: usize, mask: u8, start_nib: usize) -> bool {
        let shifted = if start_nib >= 4 { 0u8 } else { mask >> start_nib };
        if shifted == 0 {
            return false;
        }
        let nib = start_nib + shifted.trailing_zeros() as usize;
        debug_assert!(nib < 4);
        debug_assert!(mask & (1 << nib) != 0);
        self.stack.push((node_idx, mask, nib));
        if !self.trie.arena[node_idx].is_leaf(nib) {
            let addr = self.trie.arena[node_idx].children[nib].as_usize();
            self.descend_first(addr);
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
        let (_, _, nib) = self.stack.last()?;
        if *nib == usize::MAX {
            return None;
        }
        let (node_idx, _, _) = self.stack.last()?;
        let node = &self.trie.arena[*node_idx];
        if *nib == TERMINAL_NIB {
            let ki = node.leaf;
            let (off, len) = self.trie.index[ki.as_usize()];
            let key = &self.trie.buf[off..off + len.as_usize()];
            let value = &self.trie.values[ki.as_usize() - 1];
            Some((key, value))
        } else if let Some(key_index) = node.leaf_key_index(*nib) {
            let key = self.trie.key_slice(key_index);
            let value = &self.trie.values[key_index.as_usize() - 1];
            Some((key, value))
        } else {
            None
        }
    }

    pub fn current_index(&self) -> Option<usize> {
        let &(_, _, nib) = self.stack.last()?;
        if nib == usize::MAX {
            return None;
        }
        let (node_idx, _, _) = self.stack.last()?;
        let node = &self.trie.arena[*node_idx];
        if nib == TERMINAL_NIB {
            Some(node.leaf.as_usize())
        } else {
            node.leaf_key_index(nib).map(|ki| ki.as_usize())
        }
    }

    #[inline]
    fn advance_next(&mut self) -> bool {
        loop {
            let (node_idx, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                if self.push_next_child(node_idx, mask, 0) {
                    return true;
                }
                continue;
            }

            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };
            if self.push_next_child(node_idx, mask, search_start) {
                return true;
            }
        }
    }

    #[inline]
    fn advance_prev(&mut self) -> bool {
        loop {
            let (node_idx, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                continue;
            }

            if nib == 0 || nib == usize::MAX {
                let node = &self.trie.arena[node_idx];
                if node.is_terminal() {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                    return true;
                }
                continue;
            }

            let mask_below = mask & ((1 << nib) - 1);
            if mask_below != 0 {
                // Highest set bit in mask_below (only bits 0-3 are valid)
                let prev_nib = (mask_below as u32).ilog2() as usize;
                self.stack.push((node_idx, mask, prev_nib));
                if !self.trie.arena[node_idx].is_leaf(prev_nib) {
                    let addr = self.trie.arena[node_idx].children[prev_nib].as_usize();
                    self.descend_last(addr);
                }
                return true;
            }

            let node = &self.trie.arena[node_idx];
            if node.is_terminal() {
                self.stack.push((node_idx, mask, TERMINAL_NIB));
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
        let mut node_idx: usize = 0;
        let max_nib = key.len() * 4;

        loop {
            let node = &self.trie.arena[node_idx];
            let mask = node.occupancy;

            if node.is_terminal() && node.prefix_len.as_usize() >= max_nib {
                let ki = node.leaf;
                let (off, len) = self.trie.index[ki.as_usize()];
                let node_key = &self.trie.buf[off..off + len.as_usize()];
                if node_key >= key {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                    return self.current();
                }
            }

            if node.prefix_len.as_usize() >= max_nib {
                if self.push_next_child(node_idx, mask, 0) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let nib = key_nib_at(key, node.prefix_len.as_usize()) as usize;
            if !node.is_occupied(nib) {
                // No child at this nibble — find next higher child, or backtrack
                if self.push_next_child(node_idx, mask, nib + 1) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            self.stack.push((node_idx, mask, nib));
            let slot = node.children[nib];
            if node.is_leaf(nib) {
                let leaf_key = self.trie.key_slice(slot);
                if leaf_key >= key {
                    return self.current();
                }
                // Leaf key < seek key: advance past it
                return self.next();
            } else {
                node_idx = slot.as_usize();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TinyTrieMap implementations
// ---------------------------------------------------------------------------

impl TinyTrieMap for NibTrie<usize> {
    fn trie_new() -> Self { Self::new() }
    fn trie_insert(&mut self, key: Vec<u8>, value: usize) { self.insert(key, value).unwrap(); }
    fn trie_get(&self, key: &[u8]) -> Option<usize> { self.get(key) }
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
    fn trie_len(&self) -> usize { self.len() }
    fn trie_optimize(&mut self) { self.optimize(); }
}

impl TinyTrieMap for NibTrie<usize, u32, u32> {
    fn trie_new() -> Self { Self::new() }
    fn trie_insert(&mut self, key: Vec<u8>, value: usize) { self.insert(key, value).unwrap(); }
    fn trie_get(&self, key: &[u8]) -> Option<usize> { self.get(key) }
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
    fn trie_len(&self) -> usize { self.len() }
    fn trie_optimize(&mut self) { self.optimize(); }
}

#[cfg(test)]
#[path = "tests/nib_trie.rs"]
mod tests;
