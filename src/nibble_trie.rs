//! Nibble Trie — a fixed-fanout radix trie indexed by nibbles (half-bytes).
//!
//! Each node has 16 child slots (one per nibble value 0–15), addressed by
//! direct indexing rather than binary search or SIMD. This trades space for
//! simplicity and lookup speed: no comparison loops, no branch misprediction
//! on the child search path.
//!
//! # Null-Terminator Contract
//!
//! Same as [`TinyTrie`]: `insert()` rejects keys containing `0x00` and appends
//! a null terminator internally. `get()` and `seek()` require null-terminated
//! input.
//!
//! # Key Index Encoding
//!
//! A dummy key (`b"\0"`) occupies `keys[0]`. Real keys start at index 1.
//! This allows 0 to be used as a sentinel for "empty" in both `children[]`
//! and the `leaf` field, eliminating +1/-1 arithmetic.

use std::fmt;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the nibble trie arena.
///
/// Layout (72 bytes):
/// - `prefix_len`: absolute nibble position of this node's discriminating
///   nibble. During lookup, use `key_nibble_at(key, prefix_len)` directly —
///   no accumulation needed.
/// - `leaf_mask`: bit N set → `children[N]` holds a leaf key index.
/// - `leaf`: key index of any descendant leaf. Set once at node creation,
///   used during insertion to find a reference key in O(1).
/// - `children`: 16 slots indexed by nibble value. `0` = empty slot.
///   For internal nodes, the value is an arena index (≥ 1).
///   For leaves (when `leaf_mask` bit is set), the value is a key index (≥ 1,
///   since `keys[0]` is a dummy).
#[repr(C)]
struct Node {
    prefix_len: u16,
    leaf_mask: u16,
    leaf: u32,
    children: [u32; 16],
}

impl Node {
    fn new() -> Self {
        Node { prefix_len: 0, leaf_mask: 0, leaf: 0, children: [0; 16] }
    }

    #[inline]
    fn is_leaf(&self, nib: usize) -> bool {
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

    /// Store a leaf key index at `nib`. Key index must be ≥ 1
    /// (keys[0] is the dummy entry).
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, key_index: u32) {
        debug_assert!(nib < 16);
        debug_assert!(key_index > 0, "key index 0 is the dummy");
        self.set_leaf(nib);
        self.children[nib] = key_index;
    }

    /// Store an arena index at `nib` (internal node reference).
    /// Arena index must be ≥ 1 (root at index 0 is never a child of another node).
    #[inline]
    fn set_internal_child(&mut self, nib: usize, arena_index: u32) {
        debug_assert!(nib < 16);
        debug_assert!(arena_index > 0);
        self.clear_leaf(nib);
        self.children[nib] = arena_index;
    }

    /// Decode a leaf child at `nib` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize) -> Option<u32> {
        debug_assert!(nib < 16);
        if self.is_leaf(nib) && self.children[nib] != 0 {
            Some(self.children[nib])
        } else {
            None
        }
    }

    /// Get the arena index of an internal child at `nib`.
    /// Returns `None` if the slot is empty or is a leaf.
    #[inline]
    fn internal_child(&self, nib: usize) -> Option<u32> {
        debug_assert!(nib < 16);
        if !self.is_leaf(nib) && self.children[nib] != 0 {
            Some(self.children[nib])
        } else {
            None
        }
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active: Vec<(usize, &str, u32)> = (0..16)
            .filter(|&n| self.children[n] != 0)
            .map(|n| {
                let tag = if self.is_leaf(n) { "L" } else { "I" };
                (n, tag, self.children[n])
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("0x{:04x}", self.leaf_mask))
            .field("leaf", &self.leaf)
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// NibbleTrie
// ---------------------------------------------------------------------------

pub struct NibbleTrie<T> {
    arena: Vec<Node>,
    keys: Vec<Vec<u8>>,
    values: Vec<T>,
}

// ---------------------------------------------------------------------------
// Divergence result
// ---------------------------------------------------------------------------

/// Outcome of comparing two keys for divergence starting from a given nibble
/// position. `from` lets callers skip already-confirmed-matching prefixes.
enum DivergeResult {
    /// The keys are identical (same nibble count, same content).
    Duplicate,
    /// The keys diverge at this nibble position, or one key is a prefix of the
    /// other (position = length of the shorter key in nibbles).
    At(usize),
}

/// Scan two keys from `from` onward to find the first diverging nibble.
fn find_divergence(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult {
    let total_a = nibble_count(key_a);
    let total_b = nibble_count(key_b);
    let mut d = from;
    while d < total_a && d < total_b {
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

// ---------------------------------------------------------------------------
// Nibble helpers
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
fn first_child_from(node: &Node, start: usize) -> Option<(usize, u32)> {
    (start..16).find_map(|n| {
        if node.children[n] != 0 {
            Some((n, node.children[n]))
        } else {
            None
        }
    })
}

#[inline]
fn last_child_before(node: &Node, end: usize) -> Option<(usize, u32)> {
    (0..=end.min(15)).rev().find_map(|n| {
        if node.children[n] != 0 {
            Some((n, node.children[n]))
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// NibbleTrie implementation
// ---------------------------------------------------------------------------

impl<T> NibbleTrie<T> {
    pub fn new() -> Self {
        NibbleTrie {
            arena: Vec::new(),
            keys: vec![vec![0]], // keys[0] = dummy key (null-terminated empty string)
            values: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len() - 1 // subtract dummy
    }

    pub fn is_empty(&self) -> bool {
        self.keys.len() == 1 // only the dummy
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: u32 = 0;
        loop {
            let node = &self.arena[node_idx as usize];
            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[nib];
            if slot == 0 {
                return None;
            }
            if node.is_leaf(nib) {
                let key_index = slot as usize; // direct index, no -1
                debug_assert!(key_index > 0);
                return if self.keys[key_index] == key {
                    Some(key_index)
                } else {
                    None
                };
            }
            node_idx = slot;
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1]) // values[0] corresponds to keys[1]
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

        // Real key index: keys[0] is the dummy, so real keys start at 1
        let new_index = self.keys.len();
        self.keys.push(nt_key.clone());
        self.values.push(value);

        if self.arena.is_empty() {
            let first_nib = key_nibble_at(&nt_key, 0) as usize;
            let mut root = Node::new();
            root.set_leaf_child(first_nib, new_index as u32);
            root.leaf = new_index as u32;
            self.arena.push(root);
            return Ok(new_index);
        }

        let new_key = &nt_key;
        let mut node_idx: u32 = 0;
        // Nibbles 0..confirmed-1 are known to match between new_key and any
        // key in the current subtree. Skipping them avoids re-scanning the
        // shared prefix at every level of descent.
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx as usize];
            let ref_key = &self.keys[node.leaf as usize];
            let prefix_len = node.prefix_len as usize;

            match find_divergence(new_key, ref_key, confirmed) {
                DivergeResult::Duplicate => {
                    self.keys.pop();
                    let _ = self.values.pop();
                    return Err(());
                }
                DivergeResult::At(diverge) if diverge < prefix_len => {
                    // Divergence before the discriminating nibble — split this node
                    let new_nib = key_nibble_at(new_key, diverge) as usize;
                    let ref_nib = key_nibble_at(ref_key, diverge) as usize;

                    let mut new_parent = Node::new();
                    new_parent.prefix_len = diverge as u16;
                    new_parent.set_leaf_child(new_nib, new_index as u32);
                    new_parent.leaf = new_index as u32;

                    let old_node = std::mem::replace(
                        &mut self.arena[node_idx as usize],
                        Node::new(),
                    );
                    let old_idx = self.arena.len() as u32;
                    self.arena.push(old_node);

                    self.arena[node_idx as usize] = new_parent;
                    self.arena[node_idx as usize].set_internal_child(ref_nib, old_idx);

                    return Ok(new_index);
                }
                DivergeResult::At(_) => {
                    // Divergence at or after prefix_len — follow the child
                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(new_key, prefix_len) as usize;
                    let slot = node.children[nib];

                    if slot == 0 {
                        self.arena[node_idx as usize].set_leaf_child(nib, new_index as u32);
                        return Ok(new_index);
                    }

                    if node.is_leaf(nib) {
                        let existing_key_index = slot as usize;
                        let existing_key = self.keys[existing_key_index].clone();

                        match find_divergence(new_key, &existing_key, confirmed) {
                            DivergeResult::Duplicate => {
                                self.keys.pop();
                                let _ = self.values.pop();
                                return Err(());
                            }
                            DivergeResult::At(d) => {
                                let new_nib = key_nibble_at(new_key, d) as usize;
                                let exist_nib = key_nibble_at(&existing_key, d) as usize;
                                debug_assert_ne!(new_nib, exist_nib);

                                let mut split_node = Node::new();
                                split_node.prefix_len = d as u16;
                                split_node.set_leaf_child(new_nib, new_index as u32);
                                split_node.set_leaf_child(exist_nib, existing_key_index as u32);
                                split_node.leaf = existing_key_index as u32;

                                let split_idx = self.arena.len() as u32;
                                self.arena.push(split_node);
                                self.arena[node_idx as usize].set_internal_child(nib, split_idx);

                                return Ok(new_index);
                            }
                        }
                    }

                    // Internal node — descend
                    node_idx = slot;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> NibbleIter<'_, T> {
        NibbleIter::new(self)
    }

    pub fn iter_last(&self) -> NibbleIter<'_, T> {
        NibbleIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<Vec<u8>>, Vec<T>) {
        // Skip keys[0] (dummy), strip null terminators
        let keys = self.keys.into_iter().skip(1).map(|mut k| { k.pop(); k }).collect();
        (keys, self.values)
    }
}

impl<T> Default for NibbleTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

pub struct NibbleIter<'a, T> {
    trie: &'a NibbleTrie<T>,
    /// Stack of (arena_index, nibble_position) pairs.
    /// (arena_idx, nib) means we are at child nib of node arena_idx.
    /// nib = usize::MAX is a sentinel meaning "before first child".
    stack: Vec<(u32, usize)>,
}

impl<'a, T> NibbleIter<'a, T> {
    fn new(trie: &'a NibbleTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        NibbleIter { trie, stack: vec![(0, usize::MAX)] }
    }

    fn new_last(trie: &'a NibbleTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut idx: u32 = 0;
        loop {
            let node = &trie.arena[idx as usize];
            if let Some((nib, slot)) = last_child_before(node, 15) {
                stack.push((idx, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    idx = slot;
                }
            } else {
                break;
            }
        }
        NibbleIter { trie, stack }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let (arena_idx, nib) = self.stack.last()?;
        if *nib == usize::MAX {
            return None;
        }
        let node = &self.trie.arena[*arena_idx as usize];
        if let Some(key_index) = node.leaf_key_index(*nib) {
            let key = &self.trie.keys[key_index as usize];
            let value = &self.trie.values[key_index as usize - 1]; // values[0] = keys[1]
            Some((&key[..key.len().saturating_sub(1)], value))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, nib) = self.stack.pop()?;
            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };

            if let Some((next_nib, next_slot)) =
                first_child_from(&self.trie.arena[arena_idx as usize], search_start)
            {
                self.stack.push((arena_idx, next_nib));
                if self.trie.arena[arena_idx as usize].is_leaf(next_nib) {
                    return self.current();
                } else {
                    let mut idx = next_slot;
                    loop {
                        let node = &self.trie.arena[idx as usize];
                        let (child_nib, child_slot) =
                            first_child_from(node, 0).expect("non-leaf must have children");
                        self.stack.push((idx, child_nib));
                        if node.is_leaf(child_nib) {
                            return self.current();
                        } else {
                            idx = child_slot;
                        }
                    }
                }
            }
        }
    }

    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, nib) = self.stack.pop()?;

            if nib == 0 || nib == usize::MAX {
                continue; // no previous sibling at this level
            }

            if let Some((prev_nib, prev_slot)) =
                last_child_before(&self.trie.arena[arena_idx as usize], nib - 1)
            {
                self.stack.push((arena_idx, prev_nib));
                if self.trie.arena[arena_idx as usize].is_leaf(prev_nib) {
                    return self.current();
                } else {
                    let mut idx = prev_slot;
                    loop {
                        let node = &self.trie.arena[idx as usize];
                        let (child_nib, child_slot) =
                            last_child_before(node, 15).expect("non-leaf must have children");
                        self.stack.push((idx, child_nib));
                        if node.is_leaf(child_nib) {
                            return self.current();
                        } else {
                            idx = child_slot;
                        }
                    }
                }
            }
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
            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;

            let slot = node.children[nib];
            if slot != 0 {
                self.stack.push((node_idx, nib));
                if node.is_leaf(nib) {
                    // Check if the leaf key is >= the seek key
                    let key_index = slot as usize;
                    let leaf_key = &self.trie.keys[key_index];
                    if leaf_key.as_slice() >= key {
                        return self.current();
                    }
                    // Leaf key < seek key: advance past it
                    return self.next();
                } else {
                    node_idx = slot;
                    continue;
                }
            }

            // No exact match — find next higher child
            if let Some((next_nib, _)) = first_child_from(node, nib + 1) {
                self.stack.push((node_idx, next_nib));
                if node.is_leaf(next_nib) {
                    return self.current();
                } else {
                    let mut idx = node.children[next_nib];
                    loop {
                        let n = &self.trie.arena[idx as usize];
                        let (cn, cs) =
                            first_child_from(n, 0).expect("non-leaf must have children");
                        self.stack.push((idx, cn));
                        if n.is_leaf(cn) {
                            return self.current();
                        } else {
                            idx = cs;
                        }
                    }
                }
            }

            // No child at or above nib — backtrack
            loop {
                let (parent_idx, child_nib) = self.stack.pop()?;
                let parent = &self.trie.arena[parent_idx as usize];
                if let Some((next_nib, next_slot)) = first_child_from(parent, child_nib + 1) {
                    self.stack.push((parent_idx, next_nib));
                    if parent.is_leaf(next_nib) {
                        return self.current();
                    } else {
                        let mut idx = next_slot;
                        loop {
                            let n = &self.trie.arena[idx as usize];
                            let (cn, cs) =
                                first_child_from(n, 0).expect("non-leaf must have children");
                            self.stack.push((idx, cn));
                            if n.is_leaf(cn) {
                                return self.current();
                            } else {
                                idx = cs;
                            }
                        }
                    }
                }
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
        assert_eq!(std::mem::size_of::<Node>(), 72);
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
        assert_eq!(trie.get(b"hello\0"), Some(idx));
        assert_eq!(trie.get_value(b"hello\0"), Some(&42));
        assert_eq!(trie.get(b"world\0"), None);
    }

    #[test]
    fn insert_duplicate_returns_error() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"hello".to_vec(), 1).unwrap();
        let result = trie.insert(b"hello".to_vec(), 2);
        assert_eq!(result, Err(()));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn insert_rejects_null_byte() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let result = trie.insert(b"hel\0lo".to_vec(), 1);
        assert_eq!(result, Err(()));
    }

    #[test]
    fn insert_two_keys_split_leaf() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn insert_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abcd\0"), Some(i2));
    }

    #[test]
    fn insert_reverse_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abcd\0"), Some(i1));
        assert_eq!(trie.get(b"abc\0"), Some(i2));
    }

    #[test]
    fn insert_no_common_prefix() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"xyz\0"), Some(i2));
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.get(b"abe\0"), Some(i3));
    }

    #[test]
    fn insert_single_char_keys() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        for i in 0..50 {
            let key = format!("prefix_{:02}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        for i in 0..50 {
            let key = format!("prefix_{:02}\0", i);
            assert!(trie.get(key.as_bytes()).is_some());
        }
    }

    #[test]
    fn insert_deeply_nested() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        trie.insert(b"hello".to_vec(), 1).unwrap();
        assert!(!trie.is_empty());
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn into_keys_values_roundtrip() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"def".to_vec(), 2).unwrap();
        let (keys, values) = trie.into_keys_values();
        assert_eq!(keys, vec![b"abc".to_vec(), b"def".to_vec()]);
        assert_eq!(values, vec![1, 2]);
    }

    #[test]
    fn iter_empty() {
        let trie: NibbleTrie<i32> = NibbleTrie::new();
        let mut iter = trie.iter();
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_single_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"hello".to_vec(), 42).unwrap();
        let mut iter = trie.iter();
        let (k, v) = iter.next().unwrap();
        assert_eq!(k, b"hello");
        assert_eq!(*v, 42);
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_forward() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abd\0").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_between() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc\x7f\0").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abcd".to_vec(), 2).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc\0").unwrap();
        assert_eq!(k, b"abc");
    }

    #[test]
    fn get_value_found_and_missing() {
        let mut trie: NibbleTrie<String> = NibbleTrie::new();
        trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
        assert_eq!(trie.get_value(b"hello\0"), Some(&"world".to_string()));
        assert_eq!(trie.get_value(b"world\0"), None);
    }

    #[test]
    fn iter_backward_large() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
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

    #[test]
    fn leaf_field_set_on_creation() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        // Root should have leaf field set
        let root = &trie.arena[0];
        assert_ne!(root.leaf, 0, "root leaf field should be set");
    }
}