//! Nibble Trie — a fixed-fanout radix trie indexed by nibbles (half-bytes).
//!
//! Each node has 16 child slots (one per nibble value 0–15), addressed by
//! direct indexing rather than binary search or SIMD. This trades space for
//! simplicity and lookup speed: no comparison loops, no branch misprediction
//! on the child search path.
//!
//! # Terminal Nodes
//!
//! Keys that are prefixes of other keys (e.g. "ab" in {"ab", "abc"}) are
//! represented by a `terminal` flag on the node where the key ends, rather
//! than a null-byte leaf child. This eliminates null terminators, allows
//! `0x00` bytes in keys, and makes `get()` accept plain `&[u8]`.
//!
//! # Key Index Encoding
//!
//! A dummy empty key occupies `keys[0]`. Real keys start at index 1.
//! This allows 0 to be used as a sentinel for "empty" in both `children[]`
//! and the `leaf` field, eliminating +1/-1 arithmetic.

use crate::TinyTrieMap;
use std::collections::VecDeque;
use std::{fmt, simd::{LaneCount, Simd, SupportedLaneCount, cmp::SimdPartialEq}};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the nibble trie arena.
///
/// Layout (76 bytes):
/// - `children`: 16 slots indexed by nibble value. `0` = empty slot.
///   For internal nodes, the value is an arena index (≥ 1).
///   For leaves (when `leaf_mask` bit is set), the value is a key index (≥ 1,
///   since `keys[0]` is a dummy).
/// - `prefix_len`: absolute nibble position of this node's discriminating
///   nibble. During lookup, use `key_nibble_at(key, prefix_len)` directly —
///   no accumulation needed.
/// - `leaf_mask`: bit N set → `children[N]` holds a leaf key index.
/// - `leaf`: key index of a reference key. When `terminal` is true, this is
///   the node's own key index. When false, it's a descendant leaf key index.
/// - `terminal`: true if this node represents a complete key (the key ends
///   at this node's prefix position, without needing to descend further).
#[derive(Copy, Clone)]
struct Node {
    children: [u32; 16],
    prefix_len: u16,
    leaf_mask: u16,
    leaf: u32,
    terminal: bool,
}

impl Node {
    fn new() -> Self {
        Node { prefix_len: 0, leaf_mask: 0, leaf: 0, terminal: false, children: [0; 16] }
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

    /// Compute a 16-bit mask where bit N is set if `children[N] != 0`.
    /// Uses SIMD (`u32x16` compare-against-zero → bitmask → invert) to
    /// evaluate all 16 slots in parallel.
    #[inline]
    fn children_mask(&self) -> u16 {
        crate::simd::children_mask(&self.children)
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

#[derive(Clone)]
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

/// Given two differing bytes, return the nibble index of the first divergence.
/// High nibble (bits 7–4) is checked first; if they match, the low nibble
/// (bits 3–0) diverges. Branchless: XOR → check if high nibble is zero → add 1.
#[inline]
fn diverging_nibble(xor: u8, byte_idx: usize) -> usize {
    byte_idx * 2 + ((xor >> 4 == 0) as usize)
}

fn simd_find_divergence<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult
where
    LaneCount<N>: SupportedLaneCount,
{
    let minlen = key_a.len().min(key_b.len());
    let mut i = from / 2; // byte containing nibble `from`

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

    // Scalar tail
    find_divergence(key_a, key_b, i * 2)
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

/// Find the first set bit in `mask` at or after position `start`.
/// Returns the bit position, or `None` if no such bit exists.
#[inline]
fn mask_next(mask: u16, start: usize) -> Option<usize> {
    if start >= 16 {
        return None;
    }
    let shifted = mask >> start;
    if shifted != 0 {
        Some(start + shifted.trailing_zeros() as usize)
    } else {
        None
    }
}

/// Find the last set bit in `mask` strictly before position `end`.
/// Returns the bit position, or `None` if no such bit exists.
#[inline]
fn mask_prev(mask: u16, end: usize) -> Option<usize> {
    if end == 0 {
        return None;
    }
    let below = mask & ((1u16 << end) - 1);
    if below != 0 {
        Some(15 - below.leading_zeros() as usize)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// NibbleTrie implementation
// ---------------------------------------------------------------------------

impl<T> NibbleTrie<T> {
    pub fn new() -> Self {
        NibbleTrie {
            arena: Vec::new(),
            keys: vec![vec![]], // keys[0] = dummy (empty, not null-terminated)
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
        let max_nib = key.len() * 2;
        loop {
            let node = &self.arena[node_idx as usize];
            // Key nibbles exhausted — check if this node is terminal
            if node.prefix_len as usize >= max_nib {
                if node.terminal {
                    let ki = node.leaf as usize;
                    if self.keys[ki] == key {
                        return Some(ki);
                    }
                }
                return None;
            }
            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[nib];
            if slot == 0 {
                return None;
            }
            if node.is_leaf(nib) {
                let key_index = slot as usize;
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
        // Real key index: keys[0] is the dummy, so real keys start at 1
        let new_index = self.keys.len();
        self.keys.push(key.clone());
        self.values.push(value);

        let max_nib = key.len() * 2;

        if self.arena.is_empty() {
            if max_nib == 0 {
                // Empty key — root node itself is terminal
                let mut root = Node::new();
                root.terminal = true;
                root.leaf = new_index as u32;
                self.arena.push(root);
                return Ok(new_index);
            }
            let first_nib = key_nibble_at(&key, 0) as usize;
            let mut root = Node::new();
            root.set_leaf_child(first_nib, new_index as u32);
            root.leaf = new_index as u32;
            self.arena.push(root);
            return Ok(new_index);
        }

        let mut node_idx: u32 = 0;
        // Nibbles 0..confirmed-1 are known to match between new_key and any
        // key in the current subtree. Skipping them avoids re-scanning the
        // shared prefix at every level of descent.
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx as usize];
            let ref_key = &self.keys[node.leaf as usize];
            let prefix_len = node.prefix_len as usize;

            match simd_find_divergence::<8>(&key, ref_key, confirmed) {
                DivergeResult::Duplicate => {
                    self.keys.pop();
                    let _ = self.values.pop();
                    return Err(());
                }
                DivergeResult::At(diverge) if diverge < prefix_len => {
                    // Divergence before the discriminating nibble — split this node
                    let new_nib = key_nibble_at(&key, diverge) as usize;
                    let ref_nib = key_nibble_at(ref_key, diverge) as usize;

                    let mut new_parent = Node::new();
                    new_parent.prefix_len = diverge as u16;

                    if diverge >= max_nib {
                        // New key ends at the split point — terminal
                        new_parent.terminal = true;
                        new_parent.leaf = new_index as u32;
                        new_parent.set_internal_child(ref_nib, /* filled below */ 0);
                    } else {
                        new_parent.set_leaf_child(new_nib, new_index as u32);
                        new_parent.leaf = new_index as u32;
                    }

                    let old_node = std::mem::replace(
                        &mut self.arena[node_idx as usize],
                        new_parent,
                    );
                    let old_idx = self.arena.len() as u32;
                    self.arena.push(old_node);

                    if diverge >= max_nib {
                        self.arena[node_idx as usize].set_internal_child(ref_nib, old_idx);
                    } else {
                        self.arena[node_idx as usize].set_internal_child(ref_nib, old_idx);
                    }
                    self.sort_internal_children(node_idx);

                    return Ok(new_index);
                }
                DivergeResult::At(_) => {
                    // Divergence at or after prefix_len — follow the child.
                    // But first check if the new key is a prefix that ends here.
                    if max_nib <= prefix_len {
                        // Key nibbles exhausted at this node — mark terminal
                        self.arena[node_idx as usize].terminal = true;
                        self.arena[node_idx as usize].leaf = new_index as u32;
                        return Ok(new_index);
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(&key, prefix_len) as usize;
                    let slot = node.children[nib];

                    if slot == 0 {
                        // Empty slot — new key diverges here
                        self.arena[node_idx as usize].set_leaf_child(nib, new_index as u32);
                        return Ok(new_index);
                    }

                    if node.is_leaf(nib) {
                        let existing_key_index = slot as usize;
                        let existing_key = self.keys[existing_key_index].clone();

                        match simd_find_divergence::<8>(&key, &existing_key, confirmed) {
                            DivergeResult::Duplicate => {
                                self.keys.pop();
                                let _ = self.values.pop();
                                return Err(());
                            }
                            DivergeResult::At(d) => {
                                let mut split_node = Node::new();
                                split_node.prefix_len = d as u16;

                                if d >= max_nib {
                                    // New key ends at the split point — terminal
                                    let exist_nib = key_nibble_at(&existing_key, d) as usize;
                                    split_node.terminal = true;
                                    split_node.leaf = new_index as u32;
                                    split_node.set_leaf_child(exist_nib, existing_key_index as u32);
                                } else if d >= existing_key.len() * 2 {
                                    // Existing key ends at the split point — terminal
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    split_node.terminal = true;
                                    split_node.leaf = existing_key_index as u32;
                                    split_node.set_leaf_child(new_nib, new_index as u32);
                                } else {
                                    // Neither key ends at the split point — they diverge here
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    let exist_nib = key_nibble_at(&existing_key, d) as usize;
                                    debug_assert_ne!(new_nib, exist_nib);
                                    split_node.set_leaf_child(new_nib, new_index as u32);
                                    split_node.set_leaf_child(exist_nib, existing_key_index as u32);
                                    split_node.leaf = existing_key_index as u32;
                                }

                                let split_idx = self.arena.len() as u32;
                                self.arena.push(split_node);
                                self.arena[node_idx as usize].set_internal_child(nib, split_idx);
                                self.sort_internal_children(node_idx);

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
        // Skip keys[0] (dummy)
        let keys: Vec<Vec<u8>> = self.keys.into_iter().skip(1).collect();
        (keys, self.values)
    }

    /// Swap two arena nodes and fix all parent references.
    /// After this, what was at index `a` is now at index `b` and vice versa.
    fn swap_arena(&mut self, a: u32, b: u32) {
        if a == b {
            return;
        }
        self.arena.swap(a as usize, b as usize);
        // Fix references in every node that pointed at a or b
        for node in &mut self.arena {
            for nib in 0..16 {
                if node.children[nib] != 0 && !node.is_leaf(nib) {
                    if node.children[nib] == a {
                        node.children[nib] = b;
                    } else if node.children[nib] == b {
                        node.children[nib] = a;
                    }
                }
            }
        }
    }

    /// After adding a new internal child to `node_idx`, ensure the invariant
    /// that lower nibble positions point to lower arena addresses.
    ///
    /// The new child is always at the highest arena index (just pushed). Think
    /// of it as inserting into a sorted array: rotate every internal child at a
    /// nibble higher than the insertion point one slot right (by swapping with
    /// the next-higher-nibble's arena node), then the new node fills the hole.
    fn sort_internal_children(&mut self, node_idx: u32) {
        // Collect internal children in nibble order: (nib, arena_idx)
        let mut internals: [u8; 16] = [0; 16];  // nibble values
        let mut arena_ids: [u32; 16] = [0; 16]; // corresponding arena indices
        let mut count = 0usize;
        for nib in 0u8..16 {
            if self.arena[node_idx as usize].children[nib as usize] != 0
                && !self.arena[node_idx as usize].is_leaf(nib as usize)
            {
                internals[count] = nib;
                arena_ids[count] = self.arena[node_idx as usize].children[nib as usize];
                count += 1;
            }
        }
        if count <= 1 {
            return;
        }
        // Find where the new node (highest arena index) sits in nibble order
        let max_arena_idx = (0..count).fold(0u32, |m, i| m.max(arena_ids[i]));
        let insert_pos = (0..count).find(|&i| arena_ids[i] == max_arena_idx).unwrap();
        // Rotate: swap arena nodes so that insert_pos moves to the end
        // and everything after it shifts left. Each swap_arena swaps the
        // arena node at position i with the one at i+1 in our sorted order.
        for i in insert_pos..count - 1 {
            self.swap_arena(arena_ids[i], arena_ids[i + 1]);
            // After swap, update our tracking
            let tmp = arena_ids[i];
            arena_ids[i] = arena_ids[i + 1];
            arena_ids[i + 1] = tmp;
        }
    }

    // -----------------------------------------------------------------------
    // Optimize (in-place BFS reorder)
    // -----------------------------------------------------------------------

    /// Reorder the arena in breadth-first order for cache locality.
    ///
    /// After `optimize()`, nodes are laid out so that:
    /// - Siblings (children of the same parent) are adjacent in the arena
    /// - Children are near their parent (BFS groups by depth)
    /// - The root remains at arena index 0
    ///
    /// This improves iteration performance (sequential memory access) and
    /// can improve lookup locality on deep tries.
    ///
    /// No-op for empty tries.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let n = self.arena.len();

        // Phase 1: BFS walk → build remap (old arena index → new arena index)
        let mut remap = vec![u32::MAX; n];
        let mut queue: VecDeque<u32> = VecDeque::new();
        let mut next_new_idx: u32 = 1; // 0 = root

        remap[0] = 0;
        queue.push_back(0);

        while let Some(old_idx) = queue.pop_front() {
            let node = &self.arena[old_idx as usize];
            for nib in 0..16 {
                let child = node.children[nib];
                if child == 0 || node.is_leaf(nib) {
                    continue;
                }
                if remap[child as usize] == u32::MAX {
                    remap[child as usize] = next_new_idx;
                    next_new_idx += 1;
                    queue.push_back(child);
                }
            }
        }

        debug_assert_eq!(
            next_new_idx as usize, n,
            "BFS visited {} nodes but arena has {} — unreachable nodes exist",
            next_new_idx, n
        );

        // Phase 2: In-place permutation via cycle-following
        let mut visited = vec![false; n];
        for start in 0..n {
            if visited[start] || remap[start] == start as u32 {
                visited[start] = true;
                continue;
            }

            // Carry-forward: follow the cycle from start, carrying each
            // element to its new position (remap[old] = new).
            let mut saved = self.arena[start];
            let mut prev = start;
            let mut curr = remap[start] as usize;

            loop {
                let temp = self.arena[curr];
                self.arena[curr] = saved;
                visited[prev] = true;
                saved = temp;
                prev = curr;
                if curr == start {
                    break;
                }
                curr = remap[curr] as usize;
            }
            visited[start] = true;
        }

        // Phase 3: Remap all internal children
        for node in &mut self.arena {
            for nib in 0..16 {
                if node.children[nib] != 0 && !node.is_leaf(nib) {
                    node.children[nib] = remap[node.children[nib] as usize];
                }
            }
        }
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

/// Sentinel nib value meaning "positioned at the terminal value of this node."
const TERMINAL_NIB: usize = 16;

pub struct NibbleIter<'a, T> {
    trie: &'a NibbleTrie<T>,
    /// Stack of (arena_index, children_mask, nibble_position) triples.
    ///
    /// - `arena_idx`: index into the arena (which node)
    /// - `mask`: full `children_mask()` of that node, computed once on push.
    ///   Used for O(1) sibling navigation via TZ/LZ instead of linear scan.
    /// - `nib`: current position. Values:
    ///   - `usize::MAX`: before-first (initial state, no position)
    ///   - `0..16`: positioned at child slot `nib` (may be leaf or internal)
    ///   - `TERMINAL_NIB (16)`: positioned at the node's terminal value
    stack: Vec<(u32, u16, usize)>,
}

impl<'a, T> NibbleIter<'a, T> {
    fn new(trie: &'a NibbleTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].children_mask();
        // If root is terminal, start at the terminal value.
        // Otherwise, start before the first child (usize::MAX).
        let nib = if trie.arena[0].terminal { TERMINAL_NIB } else { usize::MAX };
        NibbleIter { trie, stack: vec![(0, mask, nib)] }
    }

    fn new_last(trie: &'a NibbleTrie<T>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut idx: u32 = 0;
        loop {
            let node = &trie.arena[idx as usize];
            let mask = node.children_mask();
            if mask != 0 {
                let nib = 15 - mask.leading_zeros() as usize; // highest set bit
                stack.push((idx, mask, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    idx = node.children[nib];
                }
            } else if node.terminal {
                // Node has no children but is terminal — position at terminal
                stack.push((idx, mask, TERMINAL_NIB));
                break;
            } else {
                break;
            }
        }
        NibbleIter { trie, stack }
    }

    /// Descend from internal node `idx` to its leftmost position.
    /// If the first node encountered is terminal, position at its terminal value.
    /// Otherwise find the leftmost child.
    fn descend_first(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            if node.terminal {
                let mask = node.children_mask();
                self.stack.push((idx, mask, TERMINAL_NIB));
                return;
            }
            let mask = node.children_mask();
            if mask == 0 {
                // No children and not terminal — shouldn't happen in a well-formed trie
                return;
            }
            let nib = mask.trailing_zeros() as usize; // lowest set bit
            self.stack.push((idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                idx = node.children[nib];
            }
        }
    }

    /// Descend from internal node `idx` to its rightmost position, pushing
    /// entries onto the stack. For terminal nodes with no children, positions
    /// at the terminal value. Otherwise follows the highest child to the
    /// rightmost leaf.
    fn descend_last(&mut self, mut idx: u32) {
        loop {
            let node = &self.trie.arena[idx as usize];
            let mask = node.children_mask();
            if mask == 0 {
                if node.terminal {
                    self.stack.push((idx, mask, TERMINAL_NIB));
                }
                return;
            }
            let nib = 15 - mask.leading_zeros() as usize; // highest set bit
            self.stack.push((idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                idx = node.children[nib];
            }
        }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let &(arena_idx, _mask, nib) = self.stack.last()?;
        if nib == usize::MAX {
            return None;
        }
        let node = &self.trie.arena[arena_idx as usize];
        if nib == TERMINAL_NIB {
            // Terminal value of this node
            let key_index = node.leaf as usize;
            let key = &self.trie.keys[key_index];
            let value = &self.trie.values[key_index - 1];
            Some((key, value))
        } else if let Some(key_index) = node.leaf_key_index(nib) {
            let key = &self.trie.keys[key_index as usize];
            let value = &self.trie.values[key_index as usize - 1];
            Some((key, value))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, mask, nib) = self.stack.pop()?;

            if nib == TERMINAL_NIB {
                // After terminal value, move to first child
                let node = &self.trie.arena[arena_idx as usize];
                let mask = node.children_mask();
                if let Some(first_nib) = mask_next(mask, 0) {
                    self.stack.push((arena_idx, mask, first_nib));
                    if node.is_leaf(first_nib) {
                        return self.current();
                    } else {
                        self.descend_first(node.children[first_nib]);
                        return self.current();
                    }
                }
                // No children — backtrack (continue loop)
                continue;
            }

            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };

            if let Some(next_nib) = mask_next(mask, search_start) {
                let node = &self.trie.arena[arena_idx as usize];
                self.stack.push((arena_idx, mask, next_nib));
                if node.is_leaf(next_nib) {
                    return self.current();
                } else {
                    self.descend_first(node.children[next_nib]);
                    return self.current();
                }
            }
            // No more siblings — backtrack (continue loop)
        }
    }

    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (arena_idx, mask, nib) = self.stack.pop()?;

            if nib == TERMINAL_NIB {
                // After terminal value in backward direction — backtrack
                continue;
            }

            if nib == 0 || nib == usize::MAX {
                // No previous sibling. Check if node is terminal.
                let node = &self.trie.arena[arena_idx as usize];
                if node.terminal {
                    self.stack.push((arena_idx, mask, TERMINAL_NIB));
                    return self.current();
                }
                // Not terminal — backtrack (continue loop)
                continue;
            }

            if let Some(prev_nib) = mask_prev(mask, nib) {
                let node = &self.trie.arena[arena_idx as usize];
                self.stack.push((arena_idx, mask, prev_nib));
                if node.is_leaf(prev_nib) {
                    return self.current();
                } else {
                    self.descend_last(node.children[prev_nib]);
                    return self.current();
                }
            }
            // No previous sibling — check if node is terminal
            let node = &self.trie.arena[arena_idx as usize];
            if node.terminal {
                self.stack.push((arena_idx, mask, TERMINAL_NIB));
                return self.current();
            }
            // Not terminal — backtrack (continue loop)
        }
    }

    pub fn seek(&mut self, key: &[u8]) -> Option<(&[u8], &T)> {
        if self.trie.arena.is_empty() {
            self.stack.clear();
            return None;
        }

        self.stack.clear();
        let mut node_idx: u32 = 0;
        let max_nib = key.len() * 2;

        loop {
            let node = &self.trie.arena[node_idx as usize];
            let mask = node.children_mask();

            // Check if this node is terminal and its key >= seek key
            if node.terminal && node.prefix_len as usize >= max_nib {
                let ki = node.leaf as usize;
                if self.trie.keys[ki].as_slice() >= key {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                    return self.current();
                }
            }

            if node.prefix_len as usize >= max_nib {
                // Key exhausted, terminal key < seek key (or not terminal)
                // Look for a child at or after nibble 0
                if let Some(next_nib) = mask_next(mask, 0) {
                    self.stack.push((node_idx, mask, next_nib));
                    if node.is_leaf(next_nib) {
                        return self.current();
                    } else {
                        self.descend_first(node.children[next_nib]);
                        return self.current();
                    }
                }
                // No children — backtrack
                loop {
                    let (parent_idx, parent_mask, child_nib) = self.stack.pop()?;
                    if let Some(next_nib) = mask_next(parent_mask, child_nib + 1) {
                        self.stack.push((parent_idx, parent_mask, next_nib));
                        let parent = &self.trie.arena[parent_idx as usize];
                        if parent.is_leaf(next_nib) {
                            return self.current();
                        } else {
                            self.descend_first(parent.children[next_nib]);
                            return self.current();
                        }
                    }
                }
            }

            let nib = key_nibble_at(key, node.prefix_len as usize) as usize;
            let slot = node.children[nib];
            if slot != 0 {
                self.stack.push((node_idx, mask, nib));
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
            if let Some(next_nib) = mask_next(mask, nib + 1) {
                self.stack.push((node_idx, mask, next_nib));
                if node.is_leaf(next_nib) {
                    return self.current();
                } else {
                    self.descend_first(node.children[next_nib]);
                    return self.current();
                }
            }

            // No child at or above nib — backtrack
            loop {
                let (parent_idx, parent_mask, child_nib) = self.stack.pop()?;
                if let Some(next_nib) = mask_next(parent_mask, child_nib + 1) {
                    self.stack.push((parent_idx, parent_mask, next_nib));
                    let parent = &self.trie.arena[parent_idx as usize];
                    if parent.is_leaf(next_nib) {
                        return self.current();
                    } else {
                        self.descend_first(parent.children[next_nib]);
                        return self.current();
                    }
                }
            }
        }
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl TinyTrieMap for NibbleTrie<usize> {
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
    fn trie_len(&self) -> usize { self.len() }
    fn trie_optimize(&mut self) { self.optimize(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_size() {
        assert_eq!(std::mem::size_of::<Node>(), 76);
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
        assert_eq!(trie.get(b"hello"), Some(idx));
        assert_eq!(trie.get_value(b"hello"), Some(&42));
        assert_eq!(trie.get(b"world"), None);
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
    fn insert_null_byte_allowed() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        // Null bytes are now valid in keys
        let idx = trie.insert(b"hel\0lo".to_vec(), 1).unwrap();
        assert_eq!(trie.get(b"hel\0lo"), Some(idx));
    }

    #[test]
    fn insert_two_keys_split_leaf() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc"), Some(i1));
        assert_eq!(trie.get(b"abd"), Some(i2));
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn insert_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc"), Some(i1));
        assert_eq!(trie.get(b"abcd"), Some(i2));
    }

    #[test]
    fn insert_reverse_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abcd"), Some(i1));
        assert_eq!(trie.get(b"abc"), Some(i2));
    }

    #[test]
    fn insert_no_common_prefix() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc"), Some(i1));
        assert_eq!(trie.get(b"xyz"), Some(i2));
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"abc"), Some(i1));
        assert_eq!(trie.get(b"abd"), Some(i2));
        assert_eq!(trie.get(b"abe"), Some(i3));
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
            let key = vec![c];
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
            let key = format!("prefix_{:02}", i);
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
            assert_eq!(trie.get(&key), Some(idx));
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
        let (k, _) = iter.seek(b"abd").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_between() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abe".to_vec(), 3).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc\x7f").unwrap();
        assert_eq!(k, b"abd");
    }

    #[test]
    fn iter_seek_prefix_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abcd".to_vec(), 2).unwrap();

        let mut iter = trie.iter();
        let (k, _) = iter.seek(b"abc").unwrap();
        assert_eq!(k, b"abc");
    }

    #[test]
    fn get_value_found_and_missing() {
        let mut trie: NibbleTrie<String> = NibbleTrie::new();
        trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
        assert_eq!(trie.get_value(b"hello"), Some(&"world".to_string()));
        assert_eq!(trie.get_value(b"world"), None);
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

    // ── optimize() tests ──────────────────────────────────────────────

    #[test]
    fn optimize_empty() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.optimize();
        assert!(trie.is_empty());
    }

    #[test]
    fn optimize_single_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
        trie.optimize();
        assert_eq!(trie.get(b"hello"), Some(idx));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn optimize_preserves_lookups() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let mut indices = Vec::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            let idx = trie.insert(key.into_bytes(), i).unwrap();
            indices.push(idx);
        }
        trie.optimize();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            assert_eq!(trie.get(key.as_bytes()), Some(indices[i]),
                "lookup failed after optimize for i={}", i);
        }
        assert_eq!(trie.len(), 100);
    }

    #[test]
    fn optimize_preserves_iteration() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        for i in 0..100 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        trie.optimize();

        // Forward
        let mut it = trie.iter();
        let mut keys: Vec<Vec<u8>> = Vec::new();
        while let Some((k, _)) = it.next() {
            keys.push(k.to_vec());
        }
        assert_eq!(keys.len(), 100);
        for i in 1..keys.len() {
            assert!(keys[i] > keys[i - 1], "not sorted after optimize at index {}", i);
        }

        // Backward
        let mut it = trie.iter_last();
        keys.clear();
        loop {
            match it.current() {
                Some((k, _)) => keys.push(k.to_vec()),
                None => break,
            }
            if it.prev().is_none() { break; }
        }
        assert_eq!(keys.len(), 100);
        for i in 1..keys.len() {
            assert!(keys[i] < keys[i - 1], "not reverse-sorted after optimize at index {}", i);
        }
    }

    #[test]
    fn optimize_preserves_seek() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        for i in 0..50u32 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        trie.optimize();
        let mut it = trie.iter();
        let (k, v) = it.seek(b"key_00025").unwrap();
        assert_eq!(k, b"key_00025");
        assert_eq!(*v, 25);
    }

    #[test]
    fn optimize_idempotent() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        trie.optimize();
        let arena_len_1 = trie.arena.len();
        trie.optimize();
        let arena_len_2 = trie.arena.len();
        assert_eq!(arena_len_1, arena_len_2, "second optimize changed arena size");
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            assert!(trie.get(key.as_bytes()).is_some());
        }
    }

    #[test]
    fn optimize_byte_boundary_keys() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let mut indices = Vec::new();
        for b in 1u8..=255 {
            let idx = trie.insert(vec![b], b as i32).unwrap();
            indices.push(idx);
        }
        trie.optimize();
        for (i, b) in (1u8..=255).enumerate() {
            let key = vec![b];
            assert_eq!(trie.get(&key), Some(indices[i]),
                "lookup failed after optimize for byte {}", b);
        }
    }

    #[test]
    fn optimize_stress_1000() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let mut indices = Vec::new();
        for i in 0..1000u32 {
            let key = format!("key_{:05}", i);
            let idx = trie.insert(key.into_bytes(), i as i32).unwrap();
            indices.push(idx);
        }
        trie.optimize();
        for i in 0..1000u32 {
            let key = format!("key_{:05}", i);
            assert_eq!(trie.get(key.as_bytes()), Some(indices[i as usize]),
                "lookup failed after optimize at i={}", i);
        }
    }

    #[test]
    fn optimize_deeply_nested() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let mut key = Vec::new();
        let mut indices = Vec::new();
        for i in 0..100 {
            key.push(b'a');
            let idx = trie.insert(key.clone(), i).unwrap();
            indices.push(idx);
        }
        trie.optimize();
        for i in 0..100 {
            let mut key = vec![b'a'; i + 1];
            assert_eq!(trie.get(&key), Some(indices[i]));
        }
    }

    #[test]
    fn iter_forward_prefix_keys() {
        // "ab" < "abc" in forward order
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"ab".to_vec(), 2).unwrap();
        trie.insert(b"abd".to_vec(), 3).unwrap();

        let mut results = Vec::new();
        let mut iter = trie.iter();
        if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
        while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
        assert_eq!(results, vec![b"ab".to_vec(), b"abc".to_vec(), b"abd".to_vec()]);
    }

    #[test]
    fn iter_backward_prefix_keys() {
        // "abd" > "abc" > "ab" in backward order
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"ab".to_vec(), 2).unwrap();
        trie.insert(b"abd".to_vec(), 3).unwrap();

        let mut iter = trie.iter_last();
        let mut results = Vec::new();
        loop {
            match iter.current() {
                Some((k, _)) => results.push(k.to_vec()),
                None => break,
            }
            if iter.prev().is_none() { break; }
        }
        assert_eq!(results, vec![b"abd".to_vec(), b"abc".to_vec(), b"ab".to_vec()]);
    }

    #[test]
    fn iter_forward_empty_key() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        trie.insert(b"".to_vec(), 0).unwrap();
        trie.insert(b"abc".to_vec(), 1).unwrap();

        let mut results = Vec::new();
        let mut iter = trie.iter();
        if let Some((k, _)) = iter.current() { results.push(k.to_vec()); }
        while let Some((k, _)) = iter.next() { results.push(k.to_vec()); }
        assert_eq!(results, vec![b"".to_vec(), b"abc".to_vec()]);
    }

    #[test]
    fn optimize_preserves_terminal_flags() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"ab".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
        trie.optimize();
        assert_eq!(trie.get(b"ab"), Some(i1), "terminal key 'ab' lost after optimize");
        assert_eq!(trie.get(b"abcd"), Some(i2));
        assert_eq!(trie.len(), 2);

        // Also test reverse order
        let mut trie2: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie2.insert(b"abcd".to_vec(), 1).unwrap();
        let i2 = trie2.insert(b"ab".to_vec(), 2).unwrap();
        trie2.optimize();
        assert_eq!(trie2.get(b"abcd"), Some(i1));
        assert_eq!(trie2.get(b"ab"), Some(i2), "terminal key 'ab' lost after optimize (reverse insert)");
    }

    #[test]
    fn null_bytes_in_keys() {
        let mut trie: NibbleTrie<i32> = NibbleTrie::new();
        let i1 = trie.insert(b"a\0b".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"a\0c".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"\0".to_vec(), 3).unwrap();
        let i4 = trie.insert(b"\0\0".to_vec(), 4).unwrap();

        assert_eq!(trie.get(b"a\0b"), Some(i1));
        assert_eq!(trie.get(b"a\0c"), Some(i2));
        assert_eq!(trie.get(b"\0"), Some(i3));
        assert_eq!(trie.get(b"\0\0"), Some(i4));
        assert_eq!(trie.len(), 4);
    }
}