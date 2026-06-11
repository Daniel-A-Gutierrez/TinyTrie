//! Bit Trie — a binary radix trie indexed by individual key bits.
//!
//! Each node has exactly two children (bit 0 and bit 1), stored inline as
//! `[u32; 2]`. Because both children are always present in a binary trie,
//! there are no empty slots and no mask needed for child enumeration —
//! just `children[bit]`. A `leaf_mask: u8` (2 bits) distinguishes leaf
//! key indices from arena indices, filling padding that would otherwise
//! be wasted.
//!
//! # Null-Terminator Contract
//!
//! Same as [`NibbleTrie`]: `insert()` rejects keys containing `0x00` and
//! appends a null terminator internally. `get()` and `seek()` require
//! null-terminated input.
//!
//! # Key Index Encoding
//!
//! A dummy key (`b"\0"`) occupies `keys[0]`. Real keys start at index 1.
//! This allows 0 to be used as a sentinel for "empty" in `children[]`,
//! eliminating +1/-1 arithmetic.

use std::simd::{LaneCount, Simd, SupportedLaneCount, cmp::SimdPartialEq};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the bit trie arena.
///
/// Layout (12 bytes, u32 variant):
/// - `children`: 2 slots indexed by bit value (0 or 1). `0` = empty
///   (transient); otherwise arena index (internal) or key index (leaf).
/// - `prefix_len`: absolute bit position of the discriminating bit.
///   During lookup, `key_bit_at(key, prefix_len)` directly selects the
///   child — no accumulation across levels.
/// - `leaf_mask`: bit 0 → `children[0]` is a leaf key index;
///   bit 1 → `children[1]` is a leaf key index.
struct Node {
    children: [u32; 2],
    prefix_len: u16,
    leaf_mask: u8,
    // 1 byte padding — total 12 bytes (u32 variant)
}

impl Node {
    fn new() -> Self {
        Node {
            prefix_len: 0,
            leaf_mask: 0,
            children: [0; 2],
        }
    }

    #[inline]
    fn is_leaf(&self, bit: usize) -> bool {
        debug_assert!(bit < 2);
        (self.leaf_mask >> bit) & 1 == 1
    }

    #[inline]
    fn set_leaf(&mut self, bit: usize) {
        debug_assert!(bit < 2);
        self.leaf_mask |= 1 << bit;
    }

    #[inline]
    fn clear_leaf(&mut self, bit: usize) {
        debug_assert!(bit < 2);
        self.leaf_mask &= !(1 << bit);
    }

    /// Store a leaf key index at `bit`. Key index must be ≥ 1
    /// (keys[0] is the dummy entry).
    #[inline]
    fn set_leaf_child(&mut self, bit: usize, key_index: u32) {
        debug_assert!(bit < 2);
        debug_assert!(key_index > 0, "key index 0 is the dummy");
        self.set_leaf(bit);
        self.children[bit] = key_index;
    }

    /// Store an arena index at `bit` (internal node reference).
    /// Arena index must be ≥ 1 (root at index 0 is never a child).
    #[inline]
    fn set_internal_child(&mut self, bit: usize, arena_index: u32) {
        debug_assert!(bit < 2);
        debug_assert!(arena_index > 0);
        self.clear_leaf(bit);
        self.children[bit] = arena_index;
    }

    /// Decode a leaf child at `bit` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, bit: usize) -> Option<u32> {
        debug_assert!(bit < 2);
        if self.is_leaf(bit) && self.children[bit] != 0 {
            Some(self.children[bit])
        } else {
            None
        }
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active: Vec<(usize, &str, u32)> = (0..2)
            .filter(|&b| self.children[b] != 0)
            .map(|b| {
                let tag = if self.is_leaf(b) { "L" } else { "I" };
                (b, tag, self.children[b])
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("0b{:02b}", self.leaf_mask))
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// BitTrie
// ---------------------------------------------------------------------------

pub struct BitTrie<T> {
    arena: Vec<Node>,
    keys: Vec<Vec<u8>>,
    values: Vec<T>,
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

/// Scan two keys from `from` onward to find the first diverging bit.
#[inline]
fn find_divergence(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult {
    let total_a = bit_count(key_a);
    let total_b = bit_count(key_b);
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
where
    LaneCount<N>: SupportedLaneCount,
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

// ---------------------------------------------------------------------------
// Bit helpers
// ---------------------------------------------------------------------------

/// Extract bit at absolute position `idx` from `key`. MSB-first ordering:
/// bit 0 = MSB of byte 0, bit 7 = LSB of byte 0, bit 8 = MSB of byte 1, etc.
/// Past the end of the key, returns 0 (null terminator implicit).
#[inline]
fn key_bit_at(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 8;
    if byte_idx < key.len() {
        (key[byte_idx] >> (7 - idx % 8)) & 1
    } else {
        0
    }
}

#[inline]
fn bit_count(key: &[u8]) -> usize {
    key.len() * 8
}

/// Find any leaf key index reachable from arena node `idx`.
/// Descends via child[0] (bit 0) until a leaf is found.
/// Used during insertion to obtain a reference key for divergence comparison.
fn find_any_leaf(arena: &[Node], mut idx: u32) -> u32 {
    loop {
        let node = &arena[idx as usize];
        if node.is_leaf(0) {
            return node.children[0];
        } else if node.children[0] != 0 {
            idx = node.children[0];
        } else if node.is_leaf(1) {
            return node.children[1];
        } else {
            idx = node.children[1];
        }
    }
}

// ---------------------------------------------------------------------------
// BitTrie implementation
// ---------------------------------------------------------------------------

impl<T> BitTrie<T> {
    pub fn new() -> Self {
        BitTrie {
            arena: Vec::new(),
            keys: vec![vec![0]], // keys[0] = dummy key
            values: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.keys.len() == 1
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    #[inline]
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: u32 = 0;
        loop {
            let node = &self.arena[node_idx as usize];
            let bit = key_bit_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[bit];
            // Miss: empty child slot
            if slot == 0 {
                return None;
            }
            // Leaf hit: verify full key match
            if node.is_leaf(bit) {
                let key_index = slot as usize;
                debug_assert!(key_index > 0);
                return if self.keys[key_index] == key {
                    Some(key_index)
                } else {
                    None
                };
            }
            // Common case: internal node — descend (fallthrough)
            node_idx = slot;
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1])
    }

    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        if key.contains(&0) {
            return Err(());
        }

        let mut nt_key = key;
        nt_key.push(0);

        let new_index = self.keys.len();
        self.keys.push(nt_key.clone());
        self.values.push(value);

        if self.arena.is_empty() {
            // Single-leaf trie: no arena nodes, just store the key.
            // We need a root node that discriminates on the first bit of the key.
            // Actually, with a single key, we need a root at the first diverging
            // bit... but there's nothing to diverge from yet.
            // Handle this as a special case: create a root node.
            let first_bit = key_bit_at(&nt_key, 0) as usize;
            let mut root = Node::new();
            root.prefix_len = 0;
            root.set_leaf_child(first_bit, new_index as u32);
            self.arena.push(root);
            return Ok(new_index);
        }

        let new_key = &nt_key;
        let mut node_idx: u32 = 0;
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx as usize];
            let prefix_len = node.prefix_len as usize;
            let bit = key_bit_at(new_key, prefix_len) as usize;
            let child = node.children[bit];

            // Child slot is empty — insert leaf directly
            if child == 0 {
                self.arena[node_idx as usize].set_leaf_child(bit, new_index as u32);
                return Ok(new_index);
            }

            // Child is a leaf — split or restructure
            if node.is_leaf(bit) {
                let existing_key_index = child as usize;
                let existing_key = &self.keys[existing_key_index];

                match simd_find_divergence::<8>(new_key, existing_key, confirmed) {
                    DivergeResult::Duplicate => {
                        self.keys.pop();
                        let _ = self.values.pop();
                        return Err(());
                    }
                    DivergeResult::At(d) if d < prefix_len => {
                        // Divergence before this node's discriminating bit —
                        // need a node split (new parent), not just a leaf split.
                        // The current node becomes a child of the new parent.
                        let new_bit = key_bit_at(new_key, d) as usize;
                        let ref_bit = key_bit_at(existing_key, d) as usize;

                        let mut new_parent = Node::new();
                        new_parent.prefix_len = d as u16;
                        new_parent.set_leaf_child(new_bit, new_index as u32);

                        let old_node = std::mem::replace(
                            &mut self.arena[node_idx as usize],
                            Node::new(),
                        );
                        let old_idx = self.arena.len() as u32;
                        self.arena.push(old_node);

                        self.arena[node_idx as usize] = new_parent;
                        // The old node's child at the other bit gets the leaf;
                        // the old node itself sits at ref_bit in the new parent.
                        // But the leaf was already in the old node at `bit`.
                        // We need to wire: new_parent.children[ref_bit] = old_idx (internal)
                        self.arena[node_idx as usize].set_internal_child(ref_bit, old_idx);
                        // And put the leaf key at the other child of old_idx's path.
                        // Actually, the old node already has the leaf at children[bit].
                        // The new parent's other child (new_bit) is the new leaf.
                        // The old node sits at ref_bit.
                        // But wait — we need to ensure the leaf is reachable through the
                        // old node. The leaf was at old_node.children[bit]. Since old_node
                        // still has prefix_len=prefix_len, it will route correctly.

                        return Ok(new_index);
                    }
                    DivergeResult::At(d) => {
                        // Divergence at or after prefix_len — simple leaf split
                        let new_bit = key_bit_at(new_key, d) as usize;
                        let exist_bit = key_bit_at(existing_key, d) as usize;
                        debug_assert_ne!(new_bit, exist_bit);

                        let mut split_node = Node::new();
                        split_node.prefix_len = d as u16;
                        split_node.set_leaf_child(new_bit, new_index as u32);
                        split_node.set_leaf_child(exist_bit, existing_key_index as u32);

                        let split_idx = self.arena.len() as u32;
                        self.arena.push(split_node);
                        self.arena[node_idx as usize].set_internal_child(bit, split_idx);

                        return Ok(new_index);
                    }
                }
            }

            // Child is internal — check for divergence before prefix_len
            let ref_key_index = find_any_leaf(&self.arena, child);
            let ref_key = &self.keys[ref_key_index as usize];

            match simd_find_divergence::<8>(new_key, ref_key, confirmed) {
                DivergeResult::Duplicate => {
                    // Should not happen — duplicates caught at leaf level
                    self.keys.pop();
                    let _ = self.values.pop();
                    return Err(());
                }
                DivergeResult::At(diverge) if diverge < prefix_len => {
                    // Divergence before this node's discriminating bit — split
                    let new_bit = key_bit_at(new_key, diverge) as usize;
                    let ref_bit = key_bit_at(ref_key, diverge) as usize;

                    let mut new_parent = Node::new();
                    new_parent.prefix_len = diverge as u16;
                    new_parent.set_leaf_child(new_bit, new_index as u32);

                    let old_node = std::mem::replace(
                        &mut self.arena[node_idx as usize],
                        Node::new(),
                    );
                    let old_idx = self.arena.len() as u32;
                    self.arena.push(old_node);

                    self.arena[node_idx as usize] = new_parent;
                    self.arena[node_idx as usize].set_internal_child(ref_bit, old_idx);

                    return Ok(new_index);
                }
                DivergeResult::At(_) => {
                    // Divergence at or after prefix_len — descend
                    confirmed = prefix_len + 1;
                    node_idx = child;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> BitIter<'_, T> {
        BitIter::new(self)
    }

    pub fn iter_last(&self) -> BitIter<'_, T> {
        BitIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<Vec<u8>>, Vec<T>) {
        let keys = self.keys.into_iter().skip(1).map(|mut k| { k.pop(); k }).collect();
        (keys, self.values)
    }
}

impl<T> Default for BitTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

pub struct BitIter<'a, T> {
    trie: &'a BitTrie<T>,
    /// Stack of (arena_index, which_child) pairs.
    ///
    /// - `arena_idx`: index into the arena (which node)
    /// - `which_child`: 0 or 1, the child we descended through.
    ///   `usize::MAX` is a sentinel meaning "before first child" (initial state).
    stack: Vec<(u32, usize)>,
}

impl<'a, T> BitIter<'a, T> {
    fn new(trie: &'a BitTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return BitIter { trie, stack: Vec::new() };
        }
        BitIter { trie, stack: vec![(0, usize::MAX)] }
    }

    fn new_last(trie: &'a BitTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return BitIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut idx: u32 = 0;
        loop {
            let node = &trie.arena[idx as usize];
            // Follow child[1] (right/higher) to find the rightmost leaf
            if node.is_leaf(1) || node.children[1] != 0 {
                stack.push((idx, 1));
                if node.is_leaf(1) {
                    break;
                } else {
                    idx = node.children[1];
                }
            } else if node.is_leaf(0) || node.children[0] != 0 {
                stack.push((idx, 0));
                if node.is_leaf(0) {
                    break;
                } else {
                    idx = node.children[0];
                }
            } else {
                break; // shouldn't happen in a valid trie
            }
        }
        BitIter { trie, stack }
    }

    /// Descend from internal node `idx` to its leftmost leaf, always
    /// following child[0], pushing `(idx, 0)` entries onto the stack.
    fn descend_first(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            let bit = 0; // leftmost = child[0]
            self.stack.push((idx, bit));
            if node.is_leaf(bit) {
                return;
            } else {
                idx = node.children[bit];
            }
        }
    }

    /// Descend from internal node `idx` to its rightmost leaf, always
    /// following child[1], pushing `(idx, 1)` entries onto the stack.
    fn descend_last(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            let bit = 1; // rightmost = child[1]
            self.stack.push((idx, bit));
            if node.is_leaf(bit) {
                return;
            } else {
                idx = node.children[bit];
            }
        }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let &(arena_idx, which_child) = self.stack.last()?;
        if which_child == usize::MAX {
            return None;
        }
        let node = &self.trie.arena[arena_idx as usize];
        if let Some(key_index) = node.leaf_key_index(which_child) {
            let key = &self.trie.keys[key_index as usize];
            let value = &self.trie.values[key_index as usize - 1];
            Some((&key[..key.len().saturating_sub(1)], value))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, which_child) = self.stack.pop()?;
            let search_bit = if which_child == usize::MAX { 0 } else { which_child + 1 };

            // Try the sibling (or first child if we were at sentinel)
            if search_bit < 2 {
                let node = &self.trie.arena[arena_idx as usize];
                let child = node.children[search_bit];
                if child != 0 {
                    self.stack.push((arena_idx, search_bit));
                    if node.is_leaf(search_bit) {
                        return self.current();
                    } else {
                        self.descend_first(child);
                        return self.current();
                    }
                }
            }
            // No sibling — pop up to parent
        }
    }

    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, which_child) = self.stack.pop()?;
            if which_child == usize::MAX {
                continue;
            }

            // Try the previous sibling
            if which_child > 0 {
                let prev_bit = which_child - 1;
                let node = &self.trie.arena[arena_idx as usize];
                let child = node.children[prev_bit];
                if child != 0 {
                    self.stack.push((arena_idx, prev_bit));
                    if node.is_leaf(prev_bit) {
                        return self.current();
                    } else {
                        self.descend_last(child);
                        return self.current();
                    }
                }
            }
            // No previous sibling — pop up to parent
        }
    }

    pub fn seek(&mut self, key: &[u8]) -> Option<(&[u8], &T)> {
        if self.trie.arena.is_empty() {
            self.stack.clear();
            return None;
        }

        self.stack.clear();
        let mut node_idx: u32 = 0;

        loop {
            let node = &self.trie.arena[node_idx as usize];
            let bit = key_bit_at(key, node.prefix_len as usize) as usize;
            let child = node.children[bit];

            if child != 0 {
                self.stack.push((node_idx, bit));
                if node.is_leaf(bit) {
                    // Check if the leaf key is >= the seek key
                    let key_index = child as usize;
                    let leaf_key = &self.trie.keys[key_index];
                    if leaf_key.as_slice() >= key {
                        return self.current();
                    }
                    // Leaf key < seek key: advance past it
                    return self.next();
                } else {
                    node_idx = child;
                    continue;
                }
            }

            // No child at this bit — try the higher sibling (bit 1)
            let other_bit = 1 - bit; // flip 0↔1
            let other_child = node.children[other_bit];
            if other_child != 0 && other_bit > bit {
                self.stack.push((node_idx, other_bit));
                if node.is_leaf(other_bit) {
                    return self.current();
                } else {
                    self.descend_first(other_child);
                    return self.current();
                }
            }

            // No higher child at this level — backtrack
            loop {
                let (parent_idx, parent_bit) = self.stack.pop()?;
                if parent_bit == 0 {
                    // We came from child[0], try child[1]
                    let parent = &self.trie.arena[parent_idx as usize];
                    let sibling = parent.children[1];
                    if sibling != 0 {
                        self.stack.push((parent_idx, 1));
                        if parent.is_leaf(1) {
                            return self.current();
                        } else {
                            self.descend_first(sibling);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_size() {
        assert_eq!(std::mem::size_of::<Node>(), 12);
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
        assert_eq!(trie.get(b"hello\0"), Some(idx));
        assert_eq!(trie.get_value(b"hello\0"), Some(&42));
        assert_eq!(trie.get(b"world\0"), None);
    }

    #[test]
    fn insert_duplicate_returns_error() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"hello".to_vec(), 1).unwrap();
        let result = trie.insert(b"hello".to_vec(), 2);
        assert_eq!(result, Err(()));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn insert_rejects_null_byte() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let result = trie.insert(b"hel\0lo".to_vec(), 1);
        assert_eq!(result, Err(()));
    }

    #[test]
    fn insert_two_keys_split_leaf() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn insert_prefix_key() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abcd\0"), Some(i2));
    }

    #[test]
    fn insert_reverse_prefix_key() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abcd\0"), Some(i1));
        assert_eq!(trie.get(b"abc\0"), Some(i2));
    }

    #[test]
    fn insert_no_common_prefix() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"xyz\0"), Some(i2));
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.get(b"abe\0"), Some(i3));
    }

    #[test]
    fn insert_single_char_keys() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let mut indices = Vec::new();
        for c in b'a'..=b'f' {
            let idx = trie.insert(vec![c], c as i32).unwrap();
            indices.push(idx);
        }
        for (i, c) in (b'a'..=b'f').enumerate() {
            let key = vec![c, 0];
            assert_eq!(trie.get(&key), Some(indices[i]));
        }
    }

    #[test]
    fn insert_many_keys_same_prefix() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        for i in 0..50 {
            let key = format!("prefix_{:02}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        for i in 0..50 {
            let key = format!("prefix_{:02}\0", i);
            let result = trie.get(key.as_bytes());
            assert!(result.is_some(), "get({:?}) returned None for i={}", key, i);
        }
    }

    #[test]
    fn insert_deeply_nested() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        let mut key = Vec::new();
        for i in 0..100 {
            key.push(b'a');
            let idx = trie.insert(key.clone(), i).unwrap();
            let mut nt_key = key.clone();
            nt_key.push(0);
            assert_eq!(trie.get(&nt_key), Some(idx));
        }
    }

    #[test]
    fn len_and_is_empty() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        trie.insert(b"hello".to_vec(), 1).unwrap();
        assert!(!trie.is_empty());
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn into_keys_values_roundtrip() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"def".to_vec(), 2).unwrap();
        let (keys, values) = trie.into_keys_values();
        assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
        assert_eq!(values, vec![1, 2]);
    }

    #[test]
    fn iter_empty() {
        let trie: BitTrie<i32> = BitTrie::new();
        let mut iter = trie.iter();
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_single_key() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"hello".to_vec(), 42).unwrap();
        let mut iter = trie.iter();
        let (k, v) = iter.next().unwrap();
        assert_eq!(k, b"hello");
        assert_eq!(*v, 42);
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_forward() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut results = Vec::new();
        let mut iter = trie.iter();
        while let Some((k, _)) = iter.next() {
            results.push(k.to_vec());
        }
        assert_eq!(results, vec![b"abc", b"abd", b"abe"]);
    }

    #[test]
    fn iter_backward() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter_last();
        let mut results = Vec::new();
        loop {
            match iter.current() {
                Some((k, _)) => results.push(k.to_vec()),
                None => break,
            }
            if iter.prev().is_none() {
                break;
            }
        }
        assert_eq!(results, vec![b"abe", b"abd", b"abc"]);
    }

    #[test]
    fn iter_seek_exact() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abd\0").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_between() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc\x7f\0").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_prefix_key() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abcd".to_vec(), 2).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc\0").unwrap();
        assert_eq!(k, b"abc");
    }

    #[test]
    fn get_value_found_and_missing() {
        let mut trie: BitTrie<String> = BitTrie::new();
        trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
        assert_eq!(trie.get_value(b"hello\0"), Some(&"world".to_string()));
        assert_eq!(trie.get_value(b"world\0"), None);
    }

    #[test]
    fn iter_backward_large() {
        let mut trie: BitTrie<i32> = BitTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }

        let mut iter = trie.iter_last();
        let mut count = 0;
        let mut last_key: Vec<u8> = Vec::new();
        if let Some((k, _)) = iter.current() {
            last_key = k.to_vec();
            count += 1;
        }
        while let Some((k, _)) = iter.prev() {
            assert!(k < &last_key[..], "not descending: {:?} >= {:?}",
                String::from_utf8_lossy(k), String::from_utf8_lossy(&last_key));
            last_key = k.to_vec();
            count += 1;
        }
        assert_eq!(count, 100);
    }
}
