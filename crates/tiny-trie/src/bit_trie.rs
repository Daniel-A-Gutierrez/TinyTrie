//! Bit Trie — a binary radix trie indexed by individual key bits.
//!
//! Each node has exactly two children (bit 0 and bit 1), stored inline as
//! `[u32; 2]`. Because both children are always present in a binary trie,
//! there are no empty slots and no mask needed for child enumeration —
//! just `children[bit]`.
//!
//! # Terminal Nodes
//!
//! Keys that are prefixes of other keys (e.g. "ab" in {"ab", "abc"}) are
//! represented by a `terminal` flag on the node where the key ends, rather
//! than a null-byte leaf child. This eliminates null terminators, allows
//! `0x00` bytes in keys, and makes `get()` accept plain `&[u8]`.
//!
//! # High-Bit Leaf Encoding
//!
//! Bit 31 of each `children[i]` indicates whether the value is a leaf key
//! index (bit set) or an arena index (bit clear). Bit 31 of `leaf` indicates
//! whether the node is terminal. This packs the leaf/terminal flags into
//! existing fields, eliminating the separate `leaf_mask` byte.
//!
//! # Per-Child Prefix Lengths
//!
//! Each node stores `prefix_lens: [u16; 2]` — the prefix length (in bits) for
//! each child. The node's own prefix length comes from its parent. The root's
//! prefix length is stored in `BitTrie.root_prefix_len`.
//!
//! # Key Index Encoding
//!
//! A dummy entry at `index[0] = (0, 0)` points at `buf[0]` (empty key).
//! Real keys start at index 1. This allows 0 to be used as a sentinel for
//! "empty" in `children[]` slots.

use crate::{KeyStore, TrieKey};
use benchable_map::BenchableMap;
use std::simd::{Simd, cmp::SimdPartialEq};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Bit 31 of `children[i]` indicates the value is a leaf key index.
const LEAF_BIT: u32 = 1u32 << 31;

/// Bit 31 of `leaf` indicates the node is terminal (its own key ends here).
const TERMINAL_BIT: u32 = 1u32 << 31;

/// Sentinel for the iterator: "positioned at the terminal value of this node."
/// In a binary trie, child positions are 0 and 1, so 2 is available as a sentinel.
const TERMINAL_POS: u8 = 2;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the bit trie arena.
///
/// Layout (16 bytes):
/// - `children`: 2 slots indexed by bit value (0 or 1). Bit 31 = is_leaf;
///   bits 0-30 = key index (if leaf) or arena index (if internal). Value 0
///   means empty (transient during construction only).
/// - `prefix_lens`: per-child prefix lengths in bits. `prefix_lens[0]` is the
///   prefix length of the subtree rooted at `children[0]`, and likewise for [1].
///   The node's own prefix length comes from its parent.
/// - `leaf`: bit 31 = is_terminal (this node represents a key that ends here);
///   bits 0-30 = key index for the reference/terminal key.
struct Node {
    children: [u32; 2],
    prefix_lens: [u16; 2],
    leaf: u32,
}

impl Node {
    fn new() -> Self {
        Node {
            children: [0; 2],
            prefix_lens: [0; 2],
            leaf: 0,
        }
    }

    // -------------------------------------------------------------------
    // Child helpers
    // -------------------------------------------------------------------

    #[inline]
    fn is_leaf(&self, bit: usize) -> bool {
        debug_assert!(bit < 2);
        (self.children[bit] & LEAF_BIT) != 0
    }

    #[inline]
    fn child_index(&self, bit: usize) -> u32 {
        debug_assert!(bit < 2);
        self.children[bit] & !LEAF_BIT
    }

    #[inline]
    fn is_empty(&self, bit: usize) -> bool {
        debug_assert!(bit < 2);
        self.children[bit] == 0
    }

    /// Store a leaf key index at `bit`. Key index must be ≥ 1
    /// (index[0] is the dummy entry). Sets LEAF_BIT.
    #[inline]
    fn set_leaf_child(&mut self, bit: usize, key_index: u32, prefix_len: u16) {
        debug_assert!(bit < 2);
        debug_assert!(key_index > 0, "key index 0 is the dummy");
        self.children[bit] = key_index | LEAF_BIT;
        self.prefix_lens[bit] = prefix_len;
    }

    /// Store an arena index at `bit` (internal node reference).
    /// Arena index must be ≥ 1 (root at index 0 is never a child).
    /// Clears LEAF_BIT.
    #[inline]
    fn set_internal_child(&mut self, bit: usize, arena_index: u32, prefix_len: u16) {
        debug_assert!(bit < 2);
        debug_assert!(arena_index > 0);
        self.children[bit] = arena_index; // no LEAF_BIT
        self.prefix_lens[bit] = prefix_len;
    }

    /// Decode a leaf child at `bit` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, bit: usize) -> Option<u32> {
        debug_assert!(bit < 2);
        if self.is_leaf(bit) {
            let idx = self.child_index(bit);
            if idx != 0 {
                return Some(idx);
            }
        }
        None
    }

    // -------------------------------------------------------------------
    // Terminal helpers
    // -------------------------------------------------------------------

    #[inline]
    fn is_terminal(&self) -> bool {
        (self.leaf & TERMINAL_BIT) != 0
    }

    #[inline]
    fn set_terminal(&mut self, val: bool) {
        if val {
            self.leaf |= TERMINAL_BIT;
        } else {
            self.leaf &= !TERMINAL_BIT;
        }
    }

    /// The key index stored in `leaf` (low 31 bits). For terminal nodes,
    /// this is the node's own key. For non-terminal nodes, this is a
    /// reference key used during insertion divergence comparison.
    #[inline]
    fn leaf_key_index_val(&self) -> u32 {
        self.leaf & !TERMINAL_BIT
    }

    #[inline]
    fn set_leaf_key_index(&mut self, idx: u32) {
        debug_assert!(idx > 0, "key index 0 is the dummy");
        self.leaf = (self.leaf & TERMINAL_BIT) | idx;
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active: Vec<(usize, &str, u32, u16)> = (0..2)
            .filter(|&b| self.children[b] != 0)
            .map(|b| {
                let tag = if self.is_leaf(b) { "L" } else { "I" };
                let idx = self.child_index(b);
                (b, tag, idx, self.prefix_lens[b])
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_lens", &self.prefix_lens)
            .field("terminal", &self.is_terminal())
            .field("leaf_idx", &self.leaf_key_index_val())
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// BitTrie
// ---------------------------------------------------------------------------

pub struct BitTrie<K: TrieKey, V> {
    arena: Vec<Node>,
    keys: K::Store,
    values: Vec<V>,
    /// The root node has no parent, so its prefix_len is stored here.
    root_prefix_len: u16,
}

// ---------------------------------------------------------------------------
// Divergence result
// ---------------------------------------------------------------------------

/// Outcome of comparing two keys for divergence starting from a given bit
/// position. `from` lets callers skip already-confirmed-matching prefixes.
enum DivergeResult {
    /// The keys are identical (same bit count, same content).
    Duplicate,
    /// The keys diverge at this bit position, or one key is a prefix of the
    /// other (position = bit count of the shorter key).
    At(usize),
}

/// Bounded check: do the keys match from bit `from` to bit `to` (exclusive)?
/// Returns `true` only if all bits in [from, to) are equal in both keys AND
/// both keys are long enough to have bits in that range. If one key is a prefix
/// of the other within [from, to), returns `false`.
///
/// This is the fast path for internal node descent: we only need to confirm
/// that the new key shares the subtree prefix through the node's discriminating
/// bit, without scanning the full key.
#[inline]
fn prefix_matches(key_a: &[u8], key_b: &[u8], from: usize, to: usize) -> bool {
    // If `to` extends beyond the shorter key, one key is a prefix of the
    // other — that's a divergence, not a match for descent.
    if key_a.len() * 8 < to || key_b.len() * 8 < to {
        return false;
    }
    // Compare byte-by-byte where possible, then bit-by-bit for the tail.
    // Bits `from..to` span bytes from `from_byte` to `to_byte`.
    let from_byte = from / 8;
    let to_byte = (to + 7) / 8; // ceil(to / 8)
    let min_len = key_a.len().min(key_b.len()).min(to_byte);
    for i in from_byte..min_len {
        if key_a[i] != key_b[i] {
            // Check if the differing bit is within [from, to)
            let xor = key_a[i] ^ key_b[i];
            let first_diff_bit = i * 8 + xor.leading_zeros() as usize;
            return first_diff_bit >= to;
        }
    }
    true
}

/// Scan two keys from `from` onward to find the first diverging bit.
#[inline]
fn find_divergence(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult {
    let total_a = key_a.len() * 8;
    let total_b = key_b.len() * 8;
    let min = total_a.min(total_b);
    let mut d = from;
    while d < min {
        if key_bit_at(key_a, d) != key_bit_at(key_b, d) {
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

/// Given two differing bytes, return the bit position of the first divergence.
/// MSB-first: bit 0 = MSB of byte 0. The position of the first 1 bit in the
/// XOR gives the bit index directly (since leading_zeros counts from MSB).
#[inline]
fn diverging_bit(xor: u8, byte_idx: usize) -> usize {
    byte_idx * 8 + xor.leading_zeros() as usize
}

fn simd_find_divergence<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult
{
    let minlen = key_a.len().min(key_b.len());
    let mut i = from / 8; // byte containing bit `from`

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor = unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            return DivergeResult::At(diverging_bit(xor, diff_byte_idx));
        }
        i += N;
    }

    // Scalar tail
    find_divergence(key_a, key_b, i * 8)
}

/// SIMD-accelerated byte equality check. Returns `true` if both slices have
/// the same length and identical content.
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
    // Scalar tail
    while i < len {
        if unsafe { *a.get_unchecked(i) != *b.get_unchecked(i) } {
            return false;
        }
        i += 1;
    }
    true
}

// ---------------------------------------------------------------------------
// Bit helpers
// ---------------------------------------------------------------------------

/// Extract bit at absolute position `idx` from `key`. MSB-first ordering:
/// bit 0 = MSB of byte 0, bit 7 = LSB of byte 0, bit 8 = MSB of byte 1, etc.
/// Past the end of the key, returns 0 (implicit null terminator for ordering:
/// shorter keys sort before longer keys that extend them).
#[inline]
fn key_bit_at(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 8;
    if byte_idx < key.len() {
        (key[byte_idx] >> (7 - idx % 8)) & 1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// BitTrie implementation
// ---------------------------------------------------------------------------

impl<K: TrieKey, V> BitTrie<K, V> {
    pub fn new() -> Self {
        BitTrie {
            arena: Vec::new(),
            keys: K::Store::default(),
            values: Vec::new(),
            root_prefix_len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.len() == 0
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    #[inline]
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let max_bits = key.len() * 8;
        let mut node_idx: u32 = 0;
        let mut prefix_len = self.root_prefix_len as usize;

        loop {
            let node = &self.arena[node_idx as usize];

            // Key bits exhausted — check if this node is terminal
            if prefix_len >= max_bits {
                if node.is_terminal() {
                    let ki = node.leaf_key_index_val();
                    let key_in_buf = self.keys.key_bytes(ki);
                    if simd_eq(key_in_buf, key) {
                        return Some(ki as usize);
                    }
                }
                return None;
            }

            let bit = key_bit_at(key, prefix_len) as usize;
            let child = node.children[bit];

            // Empty child slot — no match
            if child == 0 {
                return None;
            }

            if child & LEAF_BIT != 0 {
                // Leaf — verify full key match
                let ki = child & !LEAF_BIT;
                return if simd_eq(self.keys.key_bytes(ki), key) {
                    Some(ki as usize)
                } else {
                    None
                };
            }

            // Internal node — descend
            prefix_len = node.prefix_lens[bit] as usize;
            node_idx = child;
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&V> {
        self.get(key).map(|idx| &self.values[idx - 1])
    }

    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, key: K, value: V) -> Result<usize, ()> {
        // No null byte rejection — 0x00 bytes are valid in keys.

        let new_index = self.keys.push(key);
        self.values.push(value);

        let new_key = self.keys.key_bytes(new_index);
        let max_bits = new_key.len() * 8;

        if self.arena.is_empty() {
            if max_bits == 0 {
                // Empty key — root node itself is terminal
                let mut root = Node::new();
                root.set_terminal(true);
                root.set_leaf_key_index(new_index);
                self.arena.push(root);
                self.root_prefix_len = 0;
                return Ok(new_index as usize);
            }
            // First key: create root at bit 0
            let first_bit = key_bit_at(new_key, 0) as usize;
            let mut root = Node::new();
            root.set_leaf_child(first_bit, new_index, max_bits as u16);
            root.set_leaf_key_index(new_index);
            self.arena.push(root);
            self.root_prefix_len = 0;
            return Ok(new_index as usize);
        }

        let mut node_idx: u32 = 0;
        let mut confirmed: usize = 0;
        let mut prefix_len = self.root_prefix_len as usize;
        // Track parent so we can update prefix_lens when a node is split.
        // parent_info = (parent_arena_index, which_child_bit)
        let mut parent_info: Option<(u32, usize)> = None;

        loop {
            let node = &self.arena[node_idx as usize];
            // Use leaf field for reference key (O(1), no find_any_leaf)
            let ref_ki = node.leaf_key_index_val();
            let ref_key = self.keys.key_bytes(ref_ki);

            // Fast path: bounded comparison from confirmed to prefix_len.
            // We only need to know if the new key matches the reference key
            // through this node's discriminating bit position.
            if prefix_matches(new_key, ref_key, confirmed, prefix_len) {
                // Keys match from confirmed to prefix_len — descend or handle
                // terminal/empty/leaf cases.

                // Check if the new key is a prefix that ends at this node
                if max_bits <= prefix_len {
                    // Key bits exhausted at this node — mark terminal
                    self.arena[node_idx as usize].set_terminal(true);
                    self.arena[node_idx as usize].set_leaf_key_index(new_index);
                    return Ok(new_index as usize);
                }

                let bit = key_bit_at(new_key, prefix_len) as usize;
                let child = node.children[bit];

                // Empty child slot — insert leaf directly
                if child == 0 {
                    self.arena[node_idx as usize]
                        .set_leaf_child(bit, new_index, max_bits as u16);
                    return Ok(new_index as usize);
                }

                if child & LEAF_BIT != 0 {
                    // Leaf child — need full divergence scan for the split
                    let existing_ki = child & !LEAF_BIT;
                    let existing_key = self.keys.key_bytes(existing_ki);
                    let existing_prefix = node.prefix_lens[bit];

                    match simd_find_divergence::<8>(new_key, existing_key, confirmed) {
                        DivergeResult::Duplicate => {
                            // Should not happen — caught above via prefix_matches
                            self.keys.rollback();
                            let _ = self.values.pop();
                            return Err(());
                        }
                        DivergeResult::At(d) => {
                            let mut split_node = Node::new();

                            if d >= max_bits {
                                // New key ends at the split point — terminal
                                let exist_bit = key_bit_at(existing_key, d) as usize;
                                split_node.set_terminal(true);
                                split_node.set_leaf_key_index(new_index);
                                split_node.set_leaf_child(exist_bit, existing_ki, existing_prefix);
                            } else if d >= existing_key.len() * 8 {
                                // Existing key ends at the split point — terminal
                                let new_child_bit = key_bit_at(new_key, d) as usize;
                                split_node.set_terminal(true);
                                split_node.set_leaf_key_index(existing_ki);
                                split_node.set_leaf_child(new_child_bit, new_index, max_bits as u16);
                            } else {
                                // Neither key ends at the split point
                                let new_child_bit = key_bit_at(new_key, d) as usize;
                                let exist_bit = key_bit_at(existing_key, d) as usize;
                                debug_assert_ne!(new_child_bit, exist_bit);
                                split_node.set_leaf_child(new_child_bit, new_index, max_bits as u16);
                                split_node.set_leaf_child(exist_bit, existing_ki, existing_prefix);
                                split_node.set_leaf_key_index(existing_ki);
                            }

                            let split_idx = self.arena.len() as u32;
                            self.arena.push(split_node);
                            self.arena[node_idx as usize]
                                .set_internal_child(bit, split_idx, d as u16);
                        }
                    }
                    return Ok(new_index as usize);
                }

                // Internal child — descend
                confirmed = prefix_len + 1;
                parent_info = Some((node_idx, bit));
                prefix_len = node.prefix_lens[bit] as usize;
                node_idx = child;
            } else {
                // Keys diverge before prefix_len — need the exact divergence
                // point for a node split. Full scan from confirmed.
                match simd_find_divergence::<8>(new_key, ref_key, confirmed) {
                    DivergeResult::Duplicate => {
                        // Duplicate key — roll back
                        self.keys.rollback();
                        let _ = self.values.pop();
                        return Err(());
                    }
                    DivergeResult::At(diverge) => {
                        // Divergence before this node's discriminating bit —
                        // create a new parent at the divergence point.
                        debug_assert!(diverge < prefix_len, "prefix_matches said diverge but simd found no divergence before prefix_len");
                        let new_bit = key_bit_at(new_key, diverge) as usize;
                        let ref_bit = key_bit_at(ref_key, diverge) as usize;

                        let mut new_parent = Node::new();
                        new_parent.prefix_lens[ref_bit] = prefix_len as u16; // old node's prefix_len

                        if diverge >= max_bits {
                            // New key ends at the split point — terminal
                            new_parent.set_terminal(true);
                            new_parent.set_leaf_key_index(new_index);
                        } else {
                            new_parent.set_leaf_child(new_bit, new_index, max_bits as u16);
                            new_parent.set_leaf_key_index(new_index);
                        }

                        let old_node = std::mem::replace(
                            &mut self.arena[node_idx as usize],
                            new_parent,
                        );
                        let old_idx = self.arena.len() as u32;
                        self.arena.push(old_node);

                        // Wire old node as internal child of new parent
                        self.arena[node_idx as usize]
                            .set_internal_child(ref_bit, old_idx, prefix_len as u16);

                        // Update parent's prefix_lens to reflect the new prefix_len
                        if let Some((pidx, pbit)) = parent_info {
                            self.arena[pidx as usize].prefix_lens[pbit] = diverge as u16;
                        } else {
                            // We split the root
                            self.root_prefix_len = diverge as u16;
                        }

                        return Ok(new_index as usize);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> BitIter<'_, K, V> {
        BitIter::new(self)
    }

    pub fn iter_last(&self) -> BitIter<'_, K, V> {
        BitIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<K>, Vec<V>) {
        let keys = self.keys.into_keys();
        (keys, self.values)
    }
}

impl<K: TrieKey, V> Default for BitTrie<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

pub struct BitIter<'a, K: TrieKey, V> {
    trie: &'a BitTrie<K, V>,
    /// Stack of (arena_index, which_child) pairs.
    ///
    /// - `arena_idx`: index into the arena (which node)
    /// - `which_child`: 0 or 1 for child slots, `TERMINAL_POS` (2) for
    ///   the terminal value, `u8::MAX` as a sentinel meaning "before first".
    stack: Vec<(u32, u8)>,
}

impl<'a, K: TrieKey, V> BitIter<'a, K, V> {
    fn new(trie: &'a BitTrie<K, V>) -> Self {
        if trie.arena.is_empty() {
            return BitIter { trie, stack: Vec::new() };
        }
        BitIter { trie, stack: vec![(0, u8::MAX)] }
    }

    fn new_last(trie: &'a BitTrie<K, V>) -> Self {
        if trie.arena.is_empty() {
            return BitIter { trie, stack: Vec::new() };
        }
        let mut iter = BitIter { trie, stack: Vec::new() };
        iter.descend_last(0);
        iter
    }

    /// Descend from internal node `idx` to its leftmost position.
    /// If the first node encountered is terminal, position at its terminal value.
    /// Otherwise find the leftmost child.
    fn descend_first(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            if node.is_terminal() {
                self.stack.push((idx, TERMINAL_POS));
                return;
            }
            // Find leftmost non-empty child
            if !node.is_empty(0) {
                self.stack.push((idx, 0));
                if node.is_leaf(0) {
                    return;
                } else {
                    idx = node.child_index(0);
                    continue;
                }
            }
            if !node.is_empty(1) {
                self.stack.push((idx, 1));
                if node.is_leaf(1) {
                    return;
                } else {
                    idx = node.child_index(1);
                    continue;
                }
            }
            // No children and not terminal — shouldn't happen in valid trie
            return;
        }
    }

    /// Descend from internal node `idx` to its rightmost position.
    fn descend_last(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            // Try rightmost child first
            if !node.is_empty(1) {
                self.stack.push((idx, 1));
                if node.is_leaf(1) {
                    return;
                } else {
                    idx = node.child_index(1);
                    continue;
                }
            }
            if !node.is_empty(0) {
                self.stack.push((idx, 0));
                if node.is_leaf(0) {
                    return;
                } else {
                    idx = node.child_index(0);
                    continue;
                }
            }
            // No children — terminal only
            if node.is_terminal() {
                self.stack.push((idx, TERMINAL_POS));
            }
            return;
        }
    }

    /// Return the key and value at the current cursor position.
    pub fn current(&self) -> Option<(&[u8], &V)> {
        let ki = self.current_index()?;
        let key = self.trie.keys.key_bytes(ki as u32);
        let value = &self.trie.values[ki - 1];
        Some((key, value))
    }

    /// Return just the key index at the current cursor position, skipping
    /// key buffer and value reads. Useful when only the position matters.
    pub fn current_index(&self) -> Option<usize> {
        let &(arena_idx, which) = self.stack.last()?;
        if which == u8::MAX {
            return None;
        }
        let node = &self.trie.arena[arena_idx as usize];
        if which == TERMINAL_POS {
            Some(node.leaf_key_index_val() as usize)
        } else {
            node.leaf_key_index(which as usize).map(|ki| ki as usize)
        }
    }

    /// Advance cursor to the next position. Returns `true` if positioned,
    /// `false` if exhausted. Shared navigation for `next` and `next_index`.
    #[inline]
    fn advance_next(&mut self) -> bool {
        loop {
            let (arena_idx, which) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if which == TERMINAL_POS {
                // After terminal — try children in order (bit 0, then bit 1)
                let node = &self.trie.arena[arena_idx as usize];
                if !node.is_empty(0) {
                    self.stack.push((arena_idx, 0));
                    if node.is_leaf(0) {
                        return true;
                    } else {
                        self.descend_first(node.child_index(0));
                        return true;
                    }
                }
                if !node.is_empty(1) {
                    self.stack.push((arena_idx, 1));
                    if node.is_leaf(1) {
                        return true;
                    } else {
                        self.descend_first(node.child_index(1));
                        return true;
                    }
                }
                // Terminal-only node with no children — pop up
                continue;
            }

            if which == u8::MAX {
                // Before-first — position at first entry
                let node = &self.trie.arena[arena_idx as usize];
                if node.is_terminal() {
                    self.stack.push((arena_idx, TERMINAL_POS));
                    return true;
                }
                for bit in 0..2u8 {
                    if !node.is_empty(bit as usize) {
                        self.stack.push((arena_idx, bit));
                        if node.is_leaf(bit as usize) {
                            return true;
                        } else {
                            self.descend_first(node.child_index(bit as usize));
                            return true;
                        }
                    }
                }
                continue;
            }

            // After child `which` — try the next child or pop up
            let search_bit = which as usize + 1;
            if search_bit < 2 {
                let node = &self.trie.arena[arena_idx as usize];
                if !node.is_empty(search_bit) {
                    self.stack.push((arena_idx, search_bit as u8));
                    if node.is_leaf(search_bit) {
                        return true;
                    } else {
                        self.descend_first(node.child_index(search_bit));
                        return true;
                    }
                }
            }
            // No next child at this level — pop up
        }
    }

    /// Advance cursor to the previous position. Returns `true` if positioned,
    /// `false` if exhausted. Shared navigation for `prev` and `prev_index`.
    #[inline]
    fn advance_prev(&mut self) -> bool {
        loop {
            let (arena_idx, which) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if which == TERMINAL_POS {
                // Before terminal in forward order = after terminal in backward.
                // Going backward from terminal means going to parent's previous sibling.
                continue;
            }

            if which == u8::MAX {
                continue;
            }

            let bit = which as usize;

            // Try the previous sibling
            if bit > 0 {
                let prev_bit = bit - 1;
                let node = &self.trie.arena[arena_idx as usize];
                if !node.is_empty(prev_bit) {
                    self.stack.push((arena_idx, prev_bit as u8));
                    if node.is_leaf(prev_bit) {
                        return true;
                    } else {
                        self.descend_last(node.child_index(prev_bit));
                        return true;
                    }
                }
            }

            // No previous sibling — check if this node is terminal.
            // In backward order, terminal comes before children in forward,
            // which means after children in backward.
            if bit == 0 {
                let node = &self.trie.arena[arena_idx as usize];
                if node.is_terminal() {
                    self.stack.push((arena_idx, TERMINAL_POS));
                    return true;
                }
            }

            // Pop up to parent
        }
    }

    /// Advance to the next key in sorted order, returning key and value.
    #[inline]
    pub fn next(&mut self) -> Option<(&[u8], &V)> {
        if self.advance_next() { self.current() } else { None }
    }

    /// Move to the previous key in sorted order, returning key and value.
    #[inline]
    pub fn prev(&mut self) -> Option<(&[u8], &V)> {
        if self.advance_prev() { self.current() } else { None }
    }

    /// Advance to the next key, returning only its index.
    #[inline]
    pub fn next_index(&mut self) -> Option<usize> {
        if self.advance_next() { self.current_index() } else { None }
    }

    /// Move to the previous key, returning only its index.
    #[inline]
    pub fn prev_index(&mut self) -> Option<usize> {
        if self.advance_prev() { self.current_index() } else { None }
    }

    pub fn seek(&mut self, key: &[u8]) -> Option<(&[u8], &V)> {
        if self.trie.arena.is_empty() {
            self.stack.clear();
            return None;
        }

        self.stack.clear();
        let mut node_idx: u32 = 0;
        let mut prefix_len = self.trie.root_prefix_len as usize;
        let max_bits = key.len() * 8;

        loop {
            let node = &self.trie.arena[node_idx as usize];

            // Check if key is exhausted at this node
            if prefix_len >= max_bits {
                if node.is_terminal() {
                    self.stack.push((node_idx, TERMINAL_POS));
                    return self.current();
                }
                // Key exhausted but node not terminal — find first child
                for bit in 0..2u8 {
                    if !node.is_empty(bit as usize) {
                        self.stack.push((node_idx, bit));
                        if node.is_leaf(bit as usize) {
                            return self.current();
                        } else {
                            self.descend_first(node.child_index(bit as usize));
                            return self.current();
                        }
                    }
                }
                // No children — need to advance forward
                return self.next();
            }

            let bit = key_bit_at(key, prefix_len) as usize;
            let child = node.children[bit];

            if child != 0 {
                self.stack.push((node_idx, bit as u8));
                if child & LEAF_BIT != 0 {
                    // Leaf — check if leaf key >= seek key
                    let ki = child & !LEAF_BIT;
                    let leaf_key = self.trie.keys.key_bytes(ki);
                    if leaf_key >= key {
                        return self.current();
                    }
                    // Leaf key < seek key — advance past it
                    return self.next();
                } else {
                    // Internal — descend
                    prefix_len = node.prefix_lens[bit] as usize;
                    node_idx = child;
                    continue;
                }
            }

            // No child at this bit — try the other bit (higher)
            let other_bit = 1 - bit;
            let other_child = node.children[other_bit];
            if other_child != 0 && other_bit > bit {
                self.stack.push((node_idx, other_bit as u8));
                if other_child & LEAF_BIT != 0 {
                    return self.current();
                } else {
                    self.descend_first(other_child);
                    return self.current();
                }
            }

            // Check terminal before trying to go up
            if node.is_terminal() && bit == 0 {
                // Terminal key at this node — is it >= seek key?
                let ki = node.leaf_key_index_val();
                let term_key = self.trie.keys.key_bytes(ki);
                if term_key >= key {
                    self.stack.push((node_idx, TERMINAL_POS));
                    return self.current();
                }
            }

            // No higher child at this level — backtrack
            loop {
                let (parent_idx, parent_bit) = self.stack.pop()?;
                if parent_bit == TERMINAL_POS || parent_bit == u8::MAX {
                    // After terminal or before-first — try children from parent
                    let parent = &self.trie.arena[parent_idx as usize];
                    for next_bit in 0..2u8 {
                        if !parent.is_empty(next_bit as usize) {
                            self.stack.push((parent_idx, next_bit));
                            if parent.is_leaf(next_bit as usize) {
                                return self.current();
                            } else {
                                self.descend_first(parent.child_index(next_bit as usize));
                                return self.current();
                            }
                        }
                    }
                    continue;
                }
                if parent_bit == 0 {
                    // We came from child[0], try child[1]
                    let parent = &self.trie.arena[parent_idx as usize];
                    if !parent.is_empty(1) {
                        self.stack.push((parent_idx, 1));
                        if parent.is_leaf(1) {
                            return self.current();
                        } else {
                            self.descend_first(parent.child_index(1));
                            return self.current();
                        }
                    }
                }
                // Continue backtracking
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl BenchableMap for BitTrie<Vec<u8>, usize> {
    fn map_new() -> Self { Self::new() }
    fn map_insert(&mut self, key: Vec<u8>, value: usize) { self.insert(key, value).unwrap(); }
    fn map_get(&self, key: &[u8]) -> Option<usize> { self.get(key) }
    fn map_iter_fwd(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.next() { f(k, v); }
    }
    fn map_iter_rev(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter_last();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.prev() { f(k, v); }
    }
    fn map_len(&self) -> usize { self.len() }
    // map_optimize: default no-op (BitTrie has no optimize)
}

#[cfg(test)]
#[path = "tests/bit_trie.rs"]
mod tests;
