//! Poly Trie — a graduated radix trie with adaptive node sizes.
//!
//! Nodes start as Node2 (binary) and graduate to Node4 and Node16
//! as branching increases. The maximum node width is 16 (nibble-width),
//! meaning graduation only goes up to Node16. All node types share a single
//! arena of `NodeRef` slots; child arrays are contiguous ranges allocated via
//! `Arena::alloc_n`. The discriminant in `NodeRef` encodes the radix, enabling
//! table-driven dispatch that collapses the per-variant match into a single
//! code path.
//!
//! # Null-Terminator Contract
//!
//! Same as [`BitTrie`]: `insert()` rejects keys containing `0x00` and appends
//! a null terminator internally. `get()` requires null-terminated input.
//!
//! # Key Index Encoding
//!
//! No dummy key. `keys[0]` is a real key. `NodeRef::Leaf` carries the key
//! index directly; `NodeRef::Empty` (discriminant 0) disambiguates empty slots.

use std::collections::VecDeque;

use crate::arena::Arena;
use crate::TinyTrieMap;

// ---------------------------------------------------------------------------
// NodeRef — tagged reference (8 bytes, #[repr(u8)])
// ---------------------------------------------------------------------------

/// A tagged reference to a trie node, leaf, or empty slot.
///
/// Layout (8 bytes, #[repr(u8)]):
/// - discriminant (u8): 0=Empty, 1=Leaf, 2=Node2, 3=Node4, 4=Node16
/// - padding (u8): unused
/// - prefix_len (u16): absolute bit position of the discriminating digit
/// - idx (u32): arena index (for internal nodes) or key index (for Leaf)
///
/// For internal nodes (Node2/4/16), `idx` points to the start of a
/// contiguous range of `width` `NodeRef` slots in the shared arena — one
/// child slot per radix entry. The discriminant determines the range width
/// via the [`RADIX`] and [`RADIX_BITS`] lookup tables.
///
/// `NodeRef::Empty` has discriminant 0. No reserved arena slot, no dummy key.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRef {
    Empty = 0,
    Leaf { prefix_len: u16, idx: u32 } = 1,
    Node2 { prefix_len: u16, idx: u32 } = 2,
    Node4 { prefix_len: u16, idx: u32 } = 3,
    Node16 { prefix_len: u16, idx: u32 } = 4,
}

// ---------------------------------------------------------------------------
// NodeRef constants and accessors
// ---------------------------------------------------------------------------

/// Radix (child array width) indexed by `NodeRef` discriminant.
/// `RADIX[kind as usize]` gives the number of child slots for that node type.
const RADIX: [usize; 5] = [0, 0, 2, 4, 16];

/// Number of bits in the discriminating digit, indexed by discriminant.
/// `RADIX_BITS[kind as usize]` gives the bit width (1, 2, or 4).
const RADIX_BITS: [u8; 5] = [0, 0, 1, 2, 4];

impl NodeRef {
    pub const EMPTY: NodeRef = NodeRef::Empty;

    #[inline]
    pub fn leaf(prefix_len: u16, key_idx: u32) -> NodeRef {
        NodeRef::Leaf { prefix_len, idx: key_idx }
    }

    #[inline]
    pub fn node2(prefix_len: u16, idx: u32) -> NodeRef {
        NodeRef::Node2 { prefix_len, idx }
    }

    #[inline]
    pub fn node4(prefix_len: u16, idx: u32) -> NodeRef {
        NodeRef::Node4 { prefix_len, idx }
    }

    #[inline]
    pub fn node16(prefix_len: u16, idx: u32) -> NodeRef {
        NodeRef::Node16 { prefix_len, idx }
    }

    /// Returns `true` for internal node variants (Node2/4/16).
    #[inline]
    pub fn is_internal(&self) -> bool {
        matches!(self, NodeRef::Node2 { .. } | NodeRef::Node4 { .. }
                     | NodeRef::Node16 { .. })
    }

    /// The absolute bit position of this node's discriminating digit.
    ///
    /// Returns 0 for `Empty` (which has no prefix).
    #[inline]
    pub fn prefix_len(&self) -> u16 {
        match self {
            NodeRef::Empty => 0,
            NodeRef::Leaf { prefix_len, .. } => *prefix_len,
            NodeRef::Node2 { prefix_len, .. } => *prefix_len,
            NodeRef::Node4 { prefix_len, .. } => *prefix_len,
            NodeRef::Node16 { prefix_len, .. } => *prefix_len,
        }
    }

    /// The arena index (for internal nodes) or key index (for Leaf).
    ///
    /// Returns 0 for `Empty`.
    #[inline]
    pub fn idx(&self) -> u32 {
        match self {
            NodeRef::Empty => 0,
            NodeRef::Leaf { idx, .. } => *idx,
            NodeRef::Node2 { idx, .. } => *idx,
            NodeRef::Node4 { idx, .. } => *idx,
            NodeRef::Node16 { idx, .. } => *idx,
        }
    }

    /// Returns the discriminant byte for this variant.
    /// 0=Empty, 1=Leaf, 2=Node2, 3=Node4, 4=Node16.
    #[inline]
    pub fn discriminant(&self) -> u8 {
        // SAFETY: NodeRef is #[repr(u8)], so byte 0 is the discriminant.
        unsafe { *(&raw const *self as *const u8) }
    }

    /// Remap the arena index for internal-node variants using `mapping`.
    /// Leaf and Empty pass through unchanged (their `idx` is a key index,
    /// not an arena index).
    ///
    /// `mapping[old_idx]` must be a valid new arena index for every
    /// internal-node variant encountered.
    #[inline]
    fn remap_arena_idx(self, mapping: &[u32]) -> NodeRef {
        match self {
            NodeRef::Empty | NodeRef::Leaf { .. } => self,
            NodeRef::Node2 { prefix_len, idx } => {
                NodeRef::Node2 { prefix_len, idx: mapping[idx as usize] }
            }
            NodeRef::Node4 { prefix_len, idx } => {
                NodeRef::Node4 { prefix_len, idx: mapping[idx as usize] }
            }
            NodeRef::Node16 { prefix_len, idx } => {
                NodeRef::Node16 { prefix_len, idx: mapping[idx as usize] }
            }
        }
    }

    /// Number of child slots for this internal node (2, 4, or 16).
    ///
    /// Panics for `Empty` or `Leaf`.
    #[inline]
    pub fn width(&self) -> usize {
        RADIX[self.discriminant() as usize]
    }

    /// Number of bits in this node's discriminating digit (1, 2, or 4).
    ///
    /// Panics for `Empty` or `Leaf`.
    #[inline]
    pub fn radix_bits(&self) -> u32 {
        RADIX_BITS[self.discriminant() as usize] as u32
    }
}

// ---------------------------------------------------------------------------
// StructureReport
// ---------------------------------------------------------------------------

/// Summary statistics about the composition of a `PolyTrie`.
#[derive(Debug, Clone)]
pub struct StructureReport {
    /// Total number of keys stored in the trie.
    pub total_keys: usize,
    /// Number of leaf entries (equals `total_keys`).
    pub leaves: usize,
    /// Number of Node2 (binary) internal nodes.
    pub node2: usize,
    /// Number of Node4 (2-bit digit) internal nodes.
    pub node4: usize,
    /// Number of Node16 (4-bit digit) internal nodes.
    pub node16: usize,
    /// Total internal nodes (node2 + node4 + node16).
    pub total_internal: usize,
    /// Maximum depth of the trie (0 if empty, 1 if root is a leaf).
    pub depth: usize,
    /// Total empty child slots across all internal nodes.
    pub empty_slots: usize,
}

impl std::fmt::Display for StructureReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "PolyTrie Structure Report")?;
        writeln!(f, "  Keys:    {}", self.total_keys)?;
        writeln!(f, "  Leaves:  {}", self.leaves)?;
        writeln!(f, "  Node2:   {}", self.node2)?;
        writeln!(f, "  Node4:   {}", self.node4)?;
        writeln!(f, "  Node16:  {}", self.node16)?;
        writeln!(f, "  Total internal nodes: {}", self.total_internal)?;
        writeln!(f, "  Depth: {}", self.depth)?;
        writeln!(f, "  Empty child slots: {}", self.empty_slots)?;
        Ok(())
    }
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

/// Extract `n` bits at absolute position `idx` from `key`, MSB-first.
/// `n` must be 1, 2, or 4.
/// Past the end of the key, bits are 0 (null terminator implicit).
#[inline]
fn digit_at(key: &[u8], idx: usize, n: u32) -> u8 {
    debug_assert!(n == 1 || n == 2 || n == 4);
    match n {
        1 => key_bit_at(key, idx),
        2 => {
            let b0 = key_bit_at(key, idx);
            let b1 = key_bit_at(key, idx + 1);
            (b0 << 1) | b1
        }
        4 => {
            let b0 = key_bit_at(key, idx);
            let b1 = key_bit_at(key, idx + 1);
            let b2 = key_bit_at(key, idx + 2);
            let b3 = key_bit_at(key, idx + 3);
            (b0 << 3) | (b1 << 2) | (b2 << 1) | b3
        }
        _ => unreachable!(),
    }
}

/// Total number of bits in the key (including null terminator).
#[inline]
fn bit_count(key: &[u8]) -> usize {
    key.len() * 8
}

/// Given two differing bytes, return the bit position of the first divergence.
/// MSB-first: bit 0 = MSB of byte 0.
#[inline]
fn diverging_bit(xor: u8, byte_idx: usize) -> usize {
    byte_idx * 8 + xor.leading_zeros() as usize
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

/// SIMD-accelerated divergence scan.
/// Compares keys byte-by-byte from `from`, falling back to scalar for the tail.
fn simd_find_divergence<const N: usize>(
    key_a: &[u8],
    key_b: &[u8],
    from: usize,
) -> DivergeResult
where
    LaneCount<N>: SupportedLaneCount,
{
    use std::simd::{Simd, cmp::SimdPartialEq};

    let minlen = key_a.len().min(key_b.len());
    let mut i = from / 8;

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor =
                unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            return DivergeResult::At(diverging_bit(xor, diff_byte_idx));
        }
        i += N;
    }

    find_divergence(key_a, key_b, i * 8)
}

// ---------------------------------------------------------------------------
// PolyTrie
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PolyTrie<T> {
    /// Single arena holding all child arrays. Each internal node's children
    /// occupy a contiguous range of `width` `NodeRef` slots starting at the
    /// index stored in the node's `idx` field.
    arena: Arena<NodeRef, u32>,
    keys: Vec<Vec<u8>>, // no dummy key; keys[0] is a real key
    values: Vec<T>,
    root: NodeRef, // EMPTY when trie is empty
    /// Parallel to `arena`: `ref_keys[idx]` gives the key index of the
    /// reference key for the node starting at arena slot `idx`.
    /// Non-start slots within a child array hold 0 (unused).
    ref_keys: Vec<u32>,
    len: usize,
}

use std::simd::{LaneCount, SupportedLaneCount};

impl<T> PolyTrie<T> {
    pub fn new() -> Self {
        PolyTrie {
            arena: Arena::new(),
            keys: Vec::new(),
            values: Vec::new(),
            root: NodeRef::Empty,
            ref_keys: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Allocate a contiguous range of `width` slots in the arena for a node's
    /// child array, all initialized to [`NodeRef::Empty`], and record the
    /// reference key index at the start slot.
    fn alloc_node(&mut self, width: usize, ref_key_idx: u32) -> u32 {
        let idx = self.arena.alloc_n(width, NodeRef::EMPTY);
        // Keep ref_keys in sync with arena capacity
        let cap = self.arena.capacity();
        self.ref_keys.resize(cap, 0);
        self.ref_keys[idx as usize] = ref_key_idx;
        idx
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    #[inline]
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        let mut current = self.root;
        loop {
            match current {
                NodeRef::Empty => return None,
                NodeRef::Leaf { idx, .. } => {
                    let ki = idx as usize;
                    return if self.keys[ki] == key {
                        Some(ki)
                    } else {
                        None
                    };
                }
                _ => {
                    let kind = current.discriminant();
                    let bits = RADIX_BITS[kind as usize] as u32;
                    let width = RADIX[kind as usize];
                    let prefix_len = current.prefix_len() as usize;
                    let idx = current.idx();
                    let digit = digit_at(key, prefix_len, bits) as usize;
                    let children = self.arena.get_range(idx, width);
                    current = children[digit];
                }
            }
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx])
    }

    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        if key.contains(&0) {
            return Err(());
        }

        let mut nt_key = key;
        nt_key.push(0); // null terminator

        let new_index = self.keys.len();
        self.keys.push(nt_key.clone());
        self.values.push(value);
        self.len += 1;

        // Empty trie: root is a single leaf
        if matches!(self.root, NodeRef::Empty) {
            self.root = NodeRef::leaf(0, new_index as u32);
            return Ok(new_index);
        }

        // Second insert: root is a leaf, need to create first internal node
        if let NodeRef::Leaf { idx: existing_key_idx, .. } = self.root {
            let existing_key = &self.keys[existing_key_idx as usize];
            let new_key = &nt_key;

            match simd_find_divergence::<8>(new_key, existing_key, 0) {
                DivergeResult::Duplicate => {
                    self.keys.pop();
                    let _ = self.values.pop();
                    self.len -= 1;
                    return Err(());
                }
                DivergeResult::At(d) => {
                    let new_bit = key_bit_at(new_key, d) as usize;
                    let exist_bit = key_bit_at(existing_key, d) as usize;
                    debug_assert_ne!(new_bit, exist_bit);

                    let node_idx = self.alloc_node(2, existing_key_idx);
                    let children = self.arena.get_range_mut(node_idx, 2);
                    children[new_bit] = NodeRef::leaf(d as u16, new_index as u32);
                    children[exist_bit] = NodeRef::leaf(d as u16, existing_key_idx);

                    self.root = NodeRef::node2(d as u16, node_idx);
                    // Both slots filled — try graduation
                    let mut parent_stack = vec![(self.root, new_bit)];
                    self.try_graduate(&mut parent_stack);
                    return Ok(new_index);
                }
            }
        }

        // General case: walk the trie
        let new_key = &nt_key;
        let mut current = self.root;
        let mut confirmed: usize = 0;

        // Stack for parent tracking: (parent_ref, digit_in_parent)
        let mut parent_stack: Vec<(NodeRef, usize)> = Vec::new();

        loop {
            match current {
                NodeRef::Empty => {
                    // Shouldn't happen in a well-formed trie, but handle gracefully.
                    let leaf = NodeRef::leaf(0, new_index as u32);
                    self.set_child(&parent_stack, leaf);
                    self.try_graduate(&mut parent_stack);
                    return Ok(new_index);
                }

                NodeRef::Leaf { idx: key_idx, .. } => {
                    let existing_key = &self.keys[key_idx as usize];

                    match simd_find_divergence::<8>(new_key, existing_key, confirmed) {
                        DivergeResult::Duplicate => {
                            self.keys.pop();
                            let _ = self.values.pop();
                            self.len -= 1;
                            return Err(());
                        }
                        DivergeResult::At(d) => {
                            let new_bit = key_bit_at(new_key, d) as usize;
                            let exist_bit = key_bit_at(existing_key, d) as usize;
                            debug_assert_ne!(new_bit, exist_bit);

                            let split_idx = self.alloc_node(2, key_idx);
                            let children = self.arena.get_range_mut(split_idx, 2);
                            children[new_bit] = NodeRef::leaf(d as u16, new_index as u32);
                            children[exist_bit] = NodeRef::leaf(d as u16, key_idx);

                            let split_ref = NodeRef::node2(d as u16, split_idx);

                            if parent_stack.is_empty() {
                                self.root = split_ref;
                            } else {
                                self.set_child(&parent_stack, split_ref);
                            }
                            // The new Node2 has both slots occupied (two leaves).
                            // Push it onto the stack so graduation can check it
                            // and its ancestors.
                            parent_stack.push((split_ref, new_bit));
                            self.try_graduate(&mut parent_stack);
                            return Ok(new_index);
                        }
                    }
                }

                _ => {
                    // All internal node types share a single code path.
                    let kind = current.discriminant();
                    let bits = RADIX_BITS[kind as usize] as u32;
                    let width = RADIX[kind as usize];
                    let prefix_len = current.prefix_len() as usize;
                    let idx = current.idx();
                    let digit = digit_at(new_key, prefix_len, bits) as usize;
                    let children = self.arena.get_range(idx, width);
                    let child = children[digit];

                    let ref_key_idx = self.ref_keys[idx as usize];
                    let ref_key = &self.keys[ref_key_idx as usize];

                    match simd_find_divergence::<8>(new_key, ref_key, confirmed) {
                        DivergeResult::Duplicate => {
                            self.keys.pop();
                            let _ = self.values.pop();
                            self.len -= 1;
                            return Err(());
                        }
                        DivergeResult::At(diverge) if diverge < prefix_len => {
                            // Split before this node: create a Node2 at the
                            // divergence point with the new leaf and the
                            // current node as children.
                            let new_bit = key_bit_at(new_key, diverge) as usize;
                            let ref_bit = key_bit_at(ref_key, diverge) as usize;

                            let new_parent_idx = self.alloc_node(2, ref_key_idx);
                            let parent_children = self.arena.get_range_mut(new_parent_idx, 2);
                            parent_children[new_bit] = NodeRef::leaf(diverge as u16, new_index as u32);
                            parent_children[ref_bit] = current;

                            let new_parent = NodeRef::node2(diverge as u16, new_parent_idx);

                            if parent_stack.is_empty() {
                                self.root = new_parent;
                            } else {
                                self.set_child(&parent_stack, new_parent);
                            }
                            // Push the new split node so graduation can check it.
                            // The existing node (current) is one child, the new
                            // leaf is the other — both slots occupied.
                            parent_stack.push((new_parent, new_bit));
                            self.try_graduate(&mut parent_stack);
                            return Ok(new_index);
                        }
                        DivergeResult::At(_) => {
                            confirmed = prefix_len + bits as usize;
                            parent_stack.push((current, digit));

                            if matches!(child, NodeRef::Empty) {
                                let children = self.arena.get_range_mut(idx, width);
                                children[digit] = NodeRef::leaf(prefix_len as u16, new_index as u32);
                                // A slot was just filled. Try graduation from the
                                // current node upward.
                                self.try_graduate(&mut parent_stack);
                                return Ok(new_index);
                            } else {
                                current = child;
                                continue;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Update the child pointer in the parent node at the top of the stack.
    fn set_child(&mut self, parent_stack: &[(NodeRef, usize)], child: NodeRef) {
        if parent_stack.is_empty() {
            self.root = child;
            return;
        }
        let (parent_ref, digit) = parent_stack.last().unwrap();
        let idx = parent_ref.idx();
        let width = parent_ref.width();
        self.arena.get_range_mut(idx, width)[*digit] = child;
    }

    // -----------------------------------------------------------------------
    // Graduation
    // -----------------------------------------------------------------------

    /// Attempt to graduate nodes bottom-up after an insert (aligned merging).
    ///
    /// Walks the parent stack from leaf to root. For each node that meets all
    /// graduation preconditions, promotes it to the next wider type:
    ///
    ///   Node2  → Node4   (1-bit → 2-bit, when prefix_len is even)
    ///   Node4  → Node16  (2-bit → 4-bit, when prefix_len % 4 == 0)
    ///
    /// Node16 cannot graduate further.
    ///
    /// # Aligned merging
    ///
    /// Slot placement uses structural indices: `parent_digit * child_width + child_digit`.
    /// Because the alignment invariant guarantees that child nodes dispatch at
    /// `prefix_len + radix_bits`, this mapping is bijective — no collisions possible,
    /// no key lookups needed.
    ///
    /// # Precondition for graduation
    ///
    /// 1. All child slots are occupied (no Empty children).
    /// 2. All children are the same type as the parent (Node2 merges with Node2s,
    ///    Node4 with Node4s, etc.). Leaves are always eligible.
    /// 3. `prefix_len % new_radix_bits == 0` (alignment check).
    /// 4. All internal children have `prefix_len == parent_prefix_len + radix_bits`
    ///    (i.e., they dispatch at the immediately next position).
    ///
    fn try_graduate(&mut self, parent_stack: &mut Vec<(NodeRef, usize)>) {
        // Walk from bottom (closest to leaf) to top (closest to root).
        let mut i = parent_stack.len();
        while i > 0 {
            i -= 1;
            let (node_ref, _digit) = parent_stack[i];

            // Determine graduation parameters based on current node type.
            let (cur_width, new_width, new_radix_bits, new_kind) = match node_ref {
                NodeRef::Node2 { .. } => (2, 4, 2, 3u8),   // → Node4
                NodeRef::Node4 { .. } => (4, 16, 4, 4u8),   // → Node16
                _ => continue, // Leaf, Empty, Node16: can't graduate
            };

            let prefix_len = node_ref.prefix_len() as usize;
            let idx = node_ref.idx();

            // Alignment check: prefix_len must be a multiple of the new radix width.
            if prefix_len % (new_radix_bits as usize) != 0 {
                continue;
            }

            // Read current children.
            let children: Vec<NodeRef> = {
                let c = self.arena.get_range(idx, cur_width);
                c.to_vec()
            };

            // Precondition 1: all slots occupied.
            if children.iter().any(|c| matches!(c, NodeRef::Empty)) {
                continue;
            }

            // Precondition 2: all internal children are the same type as the parent.
            // Leaves are always eligible.
            let same_type = children.iter().all(|c| match c {
                NodeRef::Empty | NodeRef::Leaf { .. } => true,
                c => c.discriminant() == node_ref.discriminant(),
            });
            if !same_type {
                continue;
            }

            // Precondition 4: all internal children dispatch at prefix_len + radix_bits.
            let child_radix_bits = RADIX_BITS[node_ref.discriminant() as usize] as usize;
            let all_at_next = children.iter().all(|c| match c {
                NodeRef::Empty | NodeRef::Leaf { .. } => true,
                c => c.prefix_len() as usize == prefix_len + child_radix_bits,
            });
            if !all_at_next {
                continue;
            }

            // Graduate: create wider node at prefix_len.
            //
            // Aligned slot mapping: for each child at slot `parent_digit`,
            // if it's a Leaf it maps to slot `parent_digit * child_width + child_digit`.
            // But since we require same-type children, each child is either:
            //   - A Leaf: occupies one slot at `parent_digit * new_radix_bits/cur_radix_bits + digit_at(key, prefix_len, new_radix_bits)`
            //     Actually with aligned merging, a leaf at parent_digit maps to
            //     `parent_digit * (new_width / cur_width)` consecutive slots starting at
            //     `parent_digit * (new_width / cur_width)`, but we need the exact sub-slot.
            //     For leaves, we still use digit_at since they don't have structural sub-slots.
            //   - An internal node of the same type: it maps to `parent_digit * factor`
            //     consecutive slots, where factor = new_width / cur_width. Its own
            //     children are already placed, so we copy them directly.
            //
            // Actually, let me reconsider. With aligned merging:
            // - Each parent slot maps to a contiguous group of `factor` slots in the wider node.
            // - A Leaf at slot `d` maps to one slot within group `d`. We need `digit_at` to
            //   determine which sub-slot within the group, since a leaf doesn't carry structural info.
            // - A same-type internal node at slot `d` has `cur_width` children that map directly
            //   to the `factor` slots of group `d`. We copy its children directly.

            let factor = new_width / cur_width; // 2 for Node2→Node4, 4 for Node4→Node16
            let ref_key_idx = self.ref_keys[idx as usize];
            let new_idx = self.alloc_node(new_width, ref_key_idx);

            // Build the new children array.
            let mut new_children: Vec<NodeRef> = vec![NodeRef::EMPTY; new_width];

            for (parent_digit, child) in children.iter().enumerate() {
                let base = parent_digit * factor;
                match *child {
                    NodeRef::Empty => unreachable!(), // checked above
                    NodeRef::Leaf { idx: key_idx, .. } => {
                        // Use digit_at to find the exact slot within the group.
                        // With alignment, this is guaranteed to land within
                        // the correct group and cannot collide.
                        let key = &self.keys[key_idx as usize];
                        let wide_digit = digit_at(key, prefix_len, new_radix_bits as u32) as usize;
                        debug_assert!(wide_digit >= base && wide_digit < base + factor,
                            "aligned slot mapping violated: wide_digit={}, base={}, factor={}",
                            wide_digit, base, factor);
                        debug_assert!(matches!(new_children[wide_digit], NodeRef::Empty),
                            "collision in aligned merging (should be impossible)");
                        new_children[wide_digit] = NodeRef::leaf(
                            (prefix_len + new_radix_bits as usize) as u16, key_idx);
                    }
                    _ => {
                        // Same-type internal node: copy its children directly.
                        let child_idx = child.idx();
                        let grandchild_width = cur_width; // same type as parent
                        let gc = self.arena.get_range(child_idx, grandchild_width);
                        for (child_digit, grandchild) in gc.iter().enumerate() {
                            if !matches!(grandchild, NodeRef::Empty) {
                                let target = base + child_digit;
                                debug_assert!(matches!(new_children[target], NodeRef::Empty),
                                    "collision in aligned merging (should be impossible)");
                                new_children[target] = *grandchild;
                            }
                        }
                        // Free the collapsed child node.
                        self.arena.free_n(child_idx, grandchild_width);
                    }
                }
            }

            // Write new children into the arena.
            let new_children_slice = self.arena.get_range_mut(new_idx, new_width);
            new_children_slice.copy_from_slice(&new_children);

            // Free the old node.
            self.arena.free_n(idx, cur_width);

            // Construct the new node reference.
            let new_ref = match new_kind {
                3 => NodeRef::node4(prefix_len as u16, new_idx),
                4 => NodeRef::node16(prefix_len as u16, new_idx),
                _ => unreachable!(),
            };

            // Update the parent's child pointer.
            if i == 0 {
                self.root = new_ref;
            } else {
                self.set_child(&parent_stack[..i], new_ref);
            }

            // Update the parent stack entry for this position.
            parent_stack[i] = (new_ref, parent_stack[i].1);
        }
    }

    // -----------------------------------------------------------------------
    // Structure report
    // -----------------------------------------------------------------------

    /// Return a summary of the trie's node-type distribution and shape.
    ///
    /// Counts are computed by a tree walk (O(nodes)).
    pub fn structure_report(&self) -> StructureReport {
        let (depth, empty_slots, node2, node4, node16) = self.walk_stats();

        StructureReport {
            total_keys: self.len,
            leaves: self.len,
            node2,
            node4,
            node16,
            total_internal: node2 + node4 + node16,
            depth,
            empty_slots,
        }
    }

    /// Walk the trie from `root`, returning (max_depth, total_empty_slots,
    /// node2, node4, node16).
    fn walk_stats(&self) -> (usize, usize, usize, usize, usize) {
        match self.root {
            NodeRef::Empty => (0, 0, 0, 0, 0),
            NodeRef::Leaf { .. } => (1, 0, 0, 0, 0),
            _ => {
                let (depth, empty, n2, n4, n16) = self.walk_stats_ref(self.root);
                // depth returned from recursive walk counts edges; root adds 1
                (depth + 1, empty, n2, n4, n16)
            }
        }
    }

    /// Recursive walk returning (depth_below, empty_slots, node2, node4, node16).
    fn walk_stats_ref(&self, node_ref: NodeRef) -> (usize, usize, usize, usize, usize) {
        match node_ref {
            NodeRef::Empty | NodeRef::Leaf { .. } => (0, 0, 0, 0, 0),
            _ => {
                let width = node_ref.width();
                let idx = node_ref.idx();
                let children = self.arena.get_range(idx, width);
                let mut max_depth = 0usize;
                let mut empty = 0usize;
                let mut n2 = 0usize;
                let mut n4 = 0usize;
                let mut n16 = 0usize;

                match node_ref {
                    NodeRef::Node2 { .. } => n2 += 1,
                    NodeRef::Node4 { .. } => n4 += 1,
                    NodeRef::Node16 { .. } => n16 += 1,
                    _ => unreachable!(),
                }

                for child in children.iter() {
                    match child {
                        NodeRef::Empty => empty += 1,
                        _ => {
                            let (d, e, c2, c4, c16) = self.walk_stats_ref(*child);
                            max_depth = max_depth.max(d);
                            empty += e;
                            n2 += c2;
                            n4 += c4;
                            n16 += c16;
                        }
                    }
                }
                (max_depth + 1, empty, n2, n4, n16)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Debug helpers
    // -----------------------------------------------------------------------

    /// Debug dump of the trie structure to stderr.
    #[cfg(debug_assertions)]
    fn dump(&self) {
        fn dump_ref<T>(trie: &PolyTrie<T>, node_ref: NodeRef, depth: usize) {
            let indent = "  ".repeat(depth);
            match node_ref {
                NodeRef::Empty => {}
                NodeRef::Leaf { prefix_len, idx } => {
                    let key = &trie.keys[idx as usize];
                    eprintln!("{indent}LEAF pfx={prefix_len} key_idx={idx} key={:?}", String::from_utf8_lossy(key));
                }
                _ => {
                    let width = node_ref.width();
                    let prefix_len = node_ref.prefix_len();
                    let idx = node_ref.idx();
                    let kind_name = match node_ref {
                        NodeRef::Node2 { .. } => "NODE2",
                        NodeRef::Node4 { .. } => "NODE4",
                        NodeRef::Node16 { .. } => "NODE16",
                        _ => unreachable!(),
                    };
                    let children = trie.arena.get_range(idx, width);
                    let mut count = 0;
                    eprintln!("{indent}{kind_name} pfx={prefix_len} idx={idx}");
                    for i in 0..width {
                        if !matches!(children[i], NodeRef::Empty) {
                            eprintln!("{indent}  [{i}]:");
                            dump_ref(trie, children[i], depth + 2);
                            count += 1;
                        }
                    }
                    if width > 4 {
                        eprintln!("{indent}  ({count} occupied)");
                    }
                }
            }
        }

        eprintln!("PolyTrie dump:");
        eprintln!("  root: {:?}", self.root);
        eprintln!("  arena: {} occupied slots", self.arena.len());
        eprintln!("  keys: {} entries", self.keys.len());

        if !matches!(self.root, NodeRef::Empty) {
            dump_ref(self, self.root, 0);
        }
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    /// Return an iterator positioned before the first key.
    ///
    /// The first call to `next()` returns the smallest key in the trie.
    pub fn iter(&self) -> PolyIter<'_, T> {
        PolyIter::new(self)
    }

    /// Return an iterator positioned at the last key.
    ///
    /// The first call to `prev()` returns the next smaller key.
    /// `current()` returns the last key immediately.
    pub fn iter_last(&self) -> PolyIter<'_, T> {
        PolyIter::new_last(self)
    }

    // -----------------------------------------------------------------------
    // Arena optimization
    // -----------------------------------------------------------------------

    /// Rebuild the arena in breadth-first order for cache locality.
    ///
    /// After `optimize()`, nodes are laid out so that:
    /// - Siblings (children of the same parent) are adjacent in the arena
    /// - Children are near their parent (BFS groups by depth)
    /// - No freed-slot gaps exist (arena is fully compact)
    ///
    /// This improves iteration performance (sequential memory access) and
    /// can improve lookup locality on deep tries.
    ///
    /// No-op for empty or single-leaf tries.
    pub fn optimize(&mut self) {
        if matches!(self.root, NodeRef::Empty | NodeRef::Leaf { .. }) {
            return;
        }

        let mut queue: VecDeque<NodeRef> = VecDeque::new();
        queue.push_back(self.root);

        // Mapping: old arena start index → new arena start index.
        // u32::MAX = unmapped (freed or never visited).
        let mut remap: Vec<u32> = vec![u32::MAX; self.arena.capacity()];

        // New arena — pre-allocate to the exact number of occupied slots.
        let total_slots = self.arena.len();
        let mut new_arena: Arena<NodeRef, u32> = Arena::with_capacity(total_slots);
        let mut new_ref_keys: Vec<u32> = Vec::with_capacity(total_slots);

        // Record of allocated nodes for the remap pass.
        let mut allocated: Vec<(u32, usize)> = Vec::new();

        // Phase 1: BFS — allocate nodes in breadth-first order.
        while let Some(node_ref) = queue.pop_front() {
            let kind = node_ref.discriminant();
            let width = RADIX[kind as usize];
            let old_idx = node_ref.idx();

            let ref_key = self.ref_keys[old_idx as usize];
            let new_idx = new_arena.alloc_n(width, NodeRef::EMPTY);
            new_ref_keys.resize(new_arena.capacity(), 0);
            new_ref_keys[new_idx as usize] = ref_key;

            remap[old_idx as usize] = new_idx;
            allocated.push((new_idx, width));

            // Copy children (with old indices), enqueue internal children.
            let old_children = self.arena.get_range(old_idx, width);
            let new_children = new_arena.get_range_mut(new_idx, width);
            for (digit, &child) in old_children.iter().enumerate() {
                new_children[digit] = child;
                if child.is_internal() {
                    queue.push_back(child);
                }
            }
        }

        // Phase 2: Remap all internal NodeRef indices in the new arena.
        for (new_idx, width) in &allocated {
            let children = new_arena.get_range_mut(*new_idx, *width);
            for child in children.iter_mut() {
                *child = child.remap_arena_idx(&remap);
            }
        }

        // Remap root.
        self.root = self.root.remap_arena_idx(&remap);

        // Swap in the new arena.
        self.arena = new_arena;
        self.ref_keys = new_ref_keys;
    }
}

impl<T> Default for PolyTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Iterator helpers
// ---------------------------------------------------------------------------

/// Compute a 16-bit occupancy mask for the children of a node.
/// Bit N is set if `children[N]` is not `NodeRef::Empty`.
/// Only the low `width` bits are meaningful.
#[inline]
fn compute_mask(children: &[NodeRef]) -> u16 {
    debug_assert!(children.len() <= 16, "compute_mask only works for widths <= 16");
    let mut mask = 0u16;
    for (i, child) in children.iter().enumerate() {
        if !matches!(child, NodeRef::Empty) {
            mask |= 1 << i;
        }
    }
    mask
}

/// Find the first set bit in `mask` at or after position `start`.
/// Returns `None` if no such bit exists.
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
/// Returns `None` if no such bit exists.
#[inline]
fn mask_prev(mask: u16, end: usize) -> Option<usize> {
    if end == 0 {
        return None;
    }
    let below = if end >= 16 {
        mask // all 16 bits are below position `end`
    } else {
        mask & ((1u16 << end) - 1)
    };
    if below != 0 {
        Some(15 - below.leading_zeros() as usize)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// PolyIter
// ---------------------------------------------------------------------------

/// A stack frame for the PolyTrie iterator.
///
/// `mask` is the occupancy bitmask (bit N set = slot N occupied).
///
/// For root-leaf position: `node` is `NodeRef::Empty` and `mask` is 0.
/// `slot == usize::MAX` means "before first" (initial state from `new()`).
#[derive(Clone, Copy)]
struct Frame {
    node: NodeRef,
    slot: usize,
    mask: u16,
}

/// Forward/backward iterator over a `PolyTrie`.
///
/// Keys are returned in sorted (lexicographic) order. The null terminator
/// is stripped from the returned key slices.
pub struct PolyIter<'a, T> {
    trie: &'a PolyTrie<T>,
    stack: Vec<Frame>,
}

impl<'a, T> PolyIter<'a, T> {
    /// Create an iterator positioned before the first key.
    fn new(trie: &'a PolyTrie<T>) -> Self {
        match trie.root {
            NodeRef::Empty => PolyIter { trie, stack: Vec::new() },
            NodeRef::Leaf { .. } => {
                // Single key: use Empty sentinel for root-leaf position.
                PolyIter { trie, stack: vec![Frame { node: NodeRef::Empty, slot: usize::MAX, mask: 0 }] }
            }
            _ => {
                let width = trie.root.width();
                let children = trie.arena.get_range(trie.root.idx(), width);
                let mask = compute_mask(children);
                PolyIter { trie, stack: vec![Frame { node: trie.root, slot: usize::MAX, mask }] }
            }
        }
    }

    /// Create an iterator positioned at the last key.
    fn new_last(trie: &'a PolyTrie<T>) -> Self {
        match trie.root {
            NodeRef::Empty => PolyIter { trie, stack: Vec::new() },
            NodeRef::Leaf { .. } => {
                PolyIter { trie, stack: vec![Frame { node: NodeRef::Empty, slot: 0, mask: 0 }] }
            }
            _ => {
                let mut iter = PolyIter { trie, stack: Vec::new() };
                iter.descend_last(trie.root);
                iter
            }
        }
    }

    /// Follow the leftmost path from `node` to a leaf, pushing frames.
    fn descend_first(&mut self, mut node: NodeRef) {
        loop {
            let kind = node.discriminant();
            let width = RADIX[kind as usize];
            let idx = node.idx();
            let children = self.trie.arena.get_range(idx, width);

            let mask = compute_mask(children);
            let first_slot = mask_next(mask, 0).expect("internal node must have at least one child");

            self.stack.push(Frame { node, slot: first_slot, mask });
            let child = children[first_slot];
            if matches!(child, NodeRef::Leaf { .. }) {
                return;
            }
            node = child;
        }
    }

    /// Follow the rightmost path from `node` to a leaf, pushing frames.
    fn descend_last(&mut self, mut node: NodeRef) {
        loop {
            let kind = node.discriminant();
            let width = RADIX[kind as usize];
            let idx = node.idx();
            let children = self.trie.arena.get_range(idx, width);

            let mask = compute_mask(children);
            let last_slot = mask_prev(mask, width).expect("internal node must have at least one child");

            self.stack.push(Frame { node, slot: last_slot, mask });
            let child = children[last_slot];
            if matches!(child, NodeRef::Leaf { .. }) {
                return;
            }
            node = child;
        }
    }

    /// Find the next occupied slot at or after `search_start` in the frame's node.
    /// Returns the slot index, or None if no slot exists.
    #[inline]
    fn find_next_slot(&self, frame: &Frame, search_start: usize) -> Option<usize> {
        mask_next(frame.mask, search_start)
    }

    /// Find the previous occupied slot strictly before `slot` in the frame's node.
    /// Returns the slot index, or None if no slot exists.
    #[inline]
    fn find_prev_slot(&self, frame: &Frame, end: usize) -> Option<usize> {
        mask_prev(frame.mask, end)
    }

    /// Return the current key/value pair, or None if before-first or exhausted.
    pub fn current(&self) -> Option<(&[u8], &T)> {
        let frame = self.stack.last()?;
        if frame.slot == usize::MAX {
            return None; // "before first" sentinel
        }

        // Handle root-leaf sentinel
        if matches!(frame.node, NodeRef::Empty) {
            if let NodeRef::Leaf { idx, .. } = self.trie.root {
                let key = &self.trie.keys[idx as usize];
                let value = &self.trie.values[idx as usize];
                return Some((&key[..key.len() - 1], value));
            }
            return None;
        }

        let width = frame.node.width();
        let children = self.trie.arena.get_range(frame.node.idx(), width);
        let child = children[frame.slot];
        if let NodeRef::Leaf { idx, .. } = child {
            let key = &self.trie.keys[idx as usize];
            let value = &self.trie.values[idx as usize];
            Some((&key[..key.len() - 1], value))
        } else {
            None // Should not happen in correct iteration
        }
    }

    /// Advance to the next key in sorted order.
    /// Returns the next key/value pair, or None if exhausted.
    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let frame = self.stack.pop()?;

            // Handle root-leaf sentinel
            if matches!(frame.node, NodeRef::Empty) {
                if frame.slot == usize::MAX {
                    // "Before first" — advance to root leaf
                    self.stack.push(Frame { node: NodeRef::Empty, slot: 0, mask: 0 });
                    return self.current();
                }
                // Already at/past root leaf — exhausted
                return None;
            }

            let search_start = if frame.slot == usize::MAX { 0 } else { frame.slot + 1 };

            if let Some(slot) = self.find_next_slot(&frame, search_start) {
                self.stack.push(Frame { node: frame.node, slot, mask: frame.mask });
                let width = frame.node.width();
                let children = self.trie.arena.get_range(frame.node.idx(), width);
                let child = children[slot];
                if matches!(child, NodeRef::Leaf { .. }) {
                    return self.current();
                } else {
                    self.descend_first(child);
                    return self.current();
                }
            }
            // No next slot at this level — continue popping (loop)
        }
    }

    /// Retreat to the previous key in sorted order.
    /// Returns the previous key/value pair, or None if exhausted.
    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let frame = self.stack.pop()?;

            // Handle root-leaf sentinel
            if matches!(frame.node, NodeRef::Empty) {
                return None; // No previous key before root leaf
            }

            if frame.slot == usize::MAX {
                continue; // "Before first" sentinel at this level, pop to parent
            }

            if let Some(slot) = self.find_prev_slot(&frame, frame.slot) {
                self.stack.push(Frame { node: frame.node, slot, mask: frame.mask });
                let width = frame.node.width();
                let children = self.trie.arena.get_range(frame.node.idx(), width);
                let child = children[slot];
                if matches!(child, NodeRef::Leaf { .. }) {
                    return self.current();
                } else {
                    self.descend_last(child);
                    return self.current();
                }
            }
            // No previous slot at this level — continue popping (loop)
        }
    }

    /// Position the iterator at the first key >= `key`.
    ///
    /// The `key` argument must be null-terminated (i.e., end with a `0x00`
    /// byte), matching the convention of `get()`. The null byte is treated
    /// as the smallest possible byte value, ensuring that seek to a prefix
    /// key (e.g., `b"abc\0"`) finds that exact key.
    ///
    /// Returns that key/value pair, or None if no such key exists.
    pub fn seek(&mut self, key: &[u8]) -> Option<(&[u8], &T)> {
        if matches!(self.trie.root, NodeRef::Empty) {
            self.stack.clear();
            return None;
        }

        self.stack.clear();

        // Handle root leaf
        if let NodeRef::Leaf { idx, .. } = self.trie.root {
            let leaf_key = &self.trie.keys[idx as usize];
            if leaf_key.as_slice() >= key {
                self.stack.push(Frame { node: NodeRef::Empty, slot: 0, mask: 0 });
                return self.current();
            } else {
                return None; // Only key in trie is < seek key
            }
        }

        // Root is an internal node
        let mut current = self.trie.root;

        loop {
            let kind = current.discriminant();
            let bits = RADIX_BITS[kind as usize] as u32;
            let width = RADIX[kind as usize];
            let idx = current.idx();
            let children = self.trie.arena.get_range(idx, width);
            let mask = compute_mask(children);

            let digit = digit_at(key, current.prefix_len() as usize, bits) as usize;

            if !matches!(children[digit], NodeRef::Empty) {
                // Exact child at this digit
                let child = children[digit];
                if let NodeRef::Leaf { idx: key_idx, .. } = child {
                    let leaf_key = &self.trie.keys[key_idx as usize];
                    if leaf_key.as_slice() >= key {
                        // Leaf is >= seek key: position here
                        self.stack.push(Frame { node: current, slot: digit, mask });
                        return self.current();
                    }
                    // Leaf key < seek key: don't push frame, advance to next
                    // sibling at this level.
                    if let Some(slot) = self.find_next_slot(&Frame { node: current, slot: digit, mask }, digit + 1) {
                        self.stack.push(Frame { node: current, slot, mask });
                        let next_child = children[slot];
                        if let NodeRef::Leaf { idx: next_key_idx, .. } = next_child {
                            let next_leaf_key = &self.trie.keys[next_key_idx as usize];
                            if next_leaf_key.as_slice() >= key {
                                return self.current();
                            }
                            // Next sibling leaf is also < seek key.
                            // Fall through to backtracking.
                        } else {
                            // Internal node: all keys in this subtree are >= the
                            // seek key at the current digit position.
                            self.descend_first(next_child);
                            return self.current();
                        }
                    }
                    // No next sibling at this level — backtrack through ancestors.
                    // The stack has frames from ancestors built during descent.
                    loop {
                        let parent_frame = self.stack.pop()?;
                        let parent_next = self.find_next_slot(
                            &parent_frame, parent_frame.slot + 1);
                        if let Some(slot) = parent_next {
                            self.stack.push(Frame { node: parent_frame.node, slot, mask: parent_frame.mask });
                            let parent_width = parent_frame.node.width();
                            let parent_children = self.trie.arena.get_range(parent_frame.node.idx(), parent_width);
                            let next_child = parent_children[slot];
                            if matches!(next_child, NodeRef::Leaf { .. }) {
                                return self.current();
                            } else {
                                self.descend_first(next_child);
                                return self.current();
                            }
                        }
                    }
                }
                // Internal: push frame and continue descending
                self.stack.push(Frame { node: current, slot: digit, mask });
                current = child;
                continue;
            }

            // No child at this digit — find next occupied slot after digit
            if let Some(slot) = self.find_next_slot(&Frame { node: current, slot: digit, mask }, digit + 1) {
                self.stack.push(Frame { node: current, slot, mask });
                let next_child = children[slot];
                if matches!(next_child, NodeRef::Leaf { .. }) {
                    return self.current();
                } else {
                    self.descend_first(next_child);
                    return self.current();
                }
            }

            // No occupied slot at or above digit — backtrack
            loop {
                let parent_frame = self.stack.pop()?;

                debug_assert!(!matches!(parent_frame.node, NodeRef::Empty),
                    "root leaf should not appear in backtracking");

                let parent_next = self.find_next_slot(&parent_frame, parent_frame.slot + 1);

                if let Some(slot) = parent_next {
                    self.stack.push(Frame { node: parent_frame.node, slot, mask: parent_frame.mask });
                    let parent_width = parent_frame.node.width();
                    let parent_children = self.trie.arena.get_range(parent_frame.node.idx(), parent_width);
                    let next_child = parent_children[slot];
                    if matches!(next_child, NodeRef::Leaf { .. }) {
                        return self.current();
                    } else {
                        self.descend_first(next_child);
                        return self.current();
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

impl TinyTrieMap for PolyTrie<usize> {
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
    fn node_ref_size() {
        assert_eq!(std::mem::size_of::<NodeRef>(), 8);
    }

    #[test]
    fn node_ref_discriminant_values() {
        // NodeRef uses #[repr(u8)] with explicit discriminants.
        // Verify discriminant values by reading byte 0 of each variant.
        fn discriminant(n: NodeRef) -> u8 {
            unsafe { *(&n as *const NodeRef as *const u8) }
        }
        assert_eq!(discriminant(NodeRef::Empty), 0);
        assert_eq!(discriminant(NodeRef::Leaf { prefix_len: 0, idx: 0 }), 1);
        assert_eq!(discriminant(NodeRef::Node2 { prefix_len: 0, idx: 0 }), 2);
        assert_eq!(discriminant(NodeRef::Node4 { prefix_len: 0, idx: 0 }), 3);
        assert_eq!(discriminant(NodeRef::Node16 { prefix_len: 0, idx: 0 }), 4);
    }

    #[test]
    fn node_ref_constructors() {
        // Verify that convenience constructors work correctly
        let leaf = NodeRef::leaf(42, 100);
        assert_eq!(leaf, NodeRef::Leaf { prefix_len: 42, idx: 100 });

        let node2 = NodeRef::node2(7, 3);
        assert_eq!(node2, NodeRef::Node2 { prefix_len: 7, idx: 3 });

        let node4 = NodeRef::node4(10, 5);
        assert_eq!(node4, NodeRef::Node4 { prefix_len: 10, idx: 5 });

        let node16 = NodeRef::node16(20, 8);
        assert_eq!(node16, NodeRef::Node16 { prefix_len: 20, idx: 8 });
    }

    #[test]
    fn node_ref_accessors() {
        // prefix_len and idx accessors
        let leaf = NodeRef::leaf(42, 100);
        assert_eq!(leaf.prefix_len(), 42);
        assert_eq!(leaf.idx(), 100);
        assert!(!leaf.is_internal());

        let node2 = NodeRef::node2(7, 3);
        assert_eq!(node2.prefix_len(), 7);
        assert_eq!(node2.idx(), 3);
        assert!(node2.is_internal());
        assert_eq!(node2.width(), 2);
        assert_eq!(node2.radix_bits(), 1);

        let node4 = NodeRef::node4(10, 5);
        assert_eq!(node4.width(), 4);
        assert_eq!(node4.radix_bits(), 2);

        let node16 = NodeRef::node16(20, 8);
        assert_eq!(node16.width(), 16);
        assert_eq!(node16.radix_bits(), 4);

        // Empty accessors
        assert_eq!(NodeRef::Empty.prefix_len(), 0);
        assert_eq!(NodeRef::Empty.idx(), 0);
        assert!(!NodeRef::Empty.is_internal());
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
        assert_eq!(trie.get(b"hello\0"), Some(idx));
        assert_eq!(trie.get_value(b"hello\0"), Some(&42));
        assert_eq!(trie.get(b"world\0"), None);
    }

    #[test]
    fn insert_two_keys_split() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.get(b"abe\0"), None);
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn insert_duplicate_returns_error() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"hello".to_vec(), 1).unwrap();
        let result = trie.insert(b"hello".to_vec(), 2);
        assert_eq!(result, Err(()));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn insert_rejects_null_byte() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let result = trie.insert(b"hel\0lo".to_vec(), 1);
        assert_eq!(result, Err(()));
    }

    #[test]
    fn insert_prefix_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abcd\0"), Some(i2));
    }

    #[test]
    fn insert_reverse_prefix_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abcd\0"), Some(i1));
        assert_eq!(trie.get(b"abc\0"), Some(i2));
    }

    #[test]
    fn insert_no_common_prefix() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"xyz\0"), Some(i2));
        assert_eq!(trie.get(b"ab\0"), None);
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(i1));
        assert_eq!(trie.get(b"abd\0"), Some(i2));
        assert_eq!(trie.get(b"abe\0"), Some(i3));
    }

    #[test]
    fn insert_many_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        for i in 0..100 {
            let key = format!("key_{:03}\0", i);
            let result = trie.get(key.as_bytes());
            assert!(result.is_some(), "get({:?}) returned None for i={}", key, i);
        }
        assert_eq!(trie.len(), 100);
    }

    #[test]
    fn len_and_is_empty() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        assert!(trie.is_empty());
        assert_eq!(trie.len(), 0);
        trie.insert(b"hello".to_vec(), 1).unwrap();
        assert!(!trie.is_empty());
        assert_eq!(trie.len(), 1);
        trie.insert(b"world".to_vec(), 2).unwrap();
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn insert_single_char_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
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
    fn insert_deeply_nested() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
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
    fn into_keys_values_roundtrip() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"def".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"abc\0"), Some(0));
        assert_eq!(trie.get(b"def\0"), Some(1));
        assert_eq!(trie.len(), 2);
    }

    #[test]
    fn arena_and_ref_keys_populated() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        // After inserting two keys, arena slots may be 2 (Node2) or 4 (Node4
        // if graduation occurred), depending on whether the Node2 was graduated.
        // Either way, ref_keys should stay in sync with arena capacity.
        assert!(trie.arena.len() >= 2);
        assert_eq!(trie.ref_keys.len(), trie.arena.capacity());
    }

    #[test]
    fn insert_stress() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let n: usize = 500;
        for i in 0..n {
            let key = format!("key_{:05}", i);
            let result = trie.insert(key.into_bytes(), i as i32);
            assert!(result.is_ok(), "insert failed at i={}", i);
        }
        assert_eq!(trie.len(), n);
        for i in 0..n {
            let key = format!("key_{:05}\0", i);
            let result = trie.get(key.as_bytes());
            assert!(result.is_some(), "get failed at i={}", i);
            assert_eq!(result.unwrap(), i);
        }
        // Non-existent keys
        assert_eq!(trie.get(b"key_99999\0"), None);
        assert_eq!(trie.get(b"aaa\0"), None);
        assert_eq!(trie.get(b"zzz\0"), None);
    }

    #[test]
    fn insert_reverse_order() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        // Insert keys in reverse order to test node splits at different positions
        for i in (0..20).rev() {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        for i in 0..20 {
            let key = format!("key_{:03}\0", i);
            assert!(trie.get(key.as_bytes()).is_some());
        }
    }

    #[test]
    fn insert_same_first_byte() {
        // Keys that all start with the same byte — tests deeper trie levels
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let keys: Vec<Vec<u8>> = vec![
            b"a".to_vec(),
            b"ab".to_vec(),
            b"abc".to_vec(),
            b"abd".to_vec(),
            b"abde".to_vec(),
            b"abdef".to_vec(),
        ];
        for (i, key) in keys.iter().enumerate() {
            trie.insert(key.clone(), i as i32).unwrap();
        }
        for (i, key) in keys.iter().enumerate() {
            let mut nt_key = key.clone();
            nt_key.push(0);
            assert_eq!(trie.get(&nt_key), Some(i));
        }
    }

    #[test]
    fn get_value_found_and_missing() {
        let mut trie: PolyTrie<String> = PolyTrie::new();
        trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
        assert_eq!(trie.get_value(b"hello\0"), Some(&"world".to_string()));
        assert_eq!(trie.get_value(b"world\0"), None);
    }

    #[test]
    fn stress_large_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        // Keys of varying lengths
        let keys = vec![
            vec![0x01],
            vec![0x01, 0x02],
            vec![0x01, 0x02, 0x03],
            vec![0x01, 0x02, 0x03, 0x04],
            vec![0x01, 0x02, 0x03, 0x04, 0x05],
            vec![0xFF],
            vec![0xFF, 0xFE],
            vec![0xFF, 0xFE, 0xFD],
        ];
        for (i, key) in keys.iter().enumerate() {
            trie.insert(key.clone(), i as i32).unwrap();
        }
        for (i, key) in keys.iter().enumerate() {
            let mut nt_key = key.clone();
            nt_key.push(0);
            assert_eq!(trie.get(&nt_key), Some(i));
        }
    }

    #[test]
    fn structure_report_empty() {
        let trie: PolyTrie<i32> = PolyTrie::new();
        let report = trie.structure_report();
        assert_eq!(report.total_keys, 0);
        assert_eq!(report.leaves, 0);
        assert_eq!(report.total_internal, 0);
        assert_eq!(report.depth, 0);
        assert_eq!(report.empty_slots, 0);
        assert_eq!(report.node2, 0);
        assert_eq!(report.node4, 0);
        assert_eq!(report.node16, 0);
    }

    #[test]
    fn structure_report_single_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"hello".to_vec(), 1).unwrap();
        let report = trie.structure_report();
        assert_eq!(report.total_keys, 1);
        assert_eq!(report.leaves, 1);
        assert_eq!(report.total_internal, 0);
        assert_eq!(report.depth, 1); // root is a leaf
        assert_eq!(report.empty_slots, 0);
    }

    #[test]
    fn structure_report_two_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        let report = trie.structure_report();
        assert_eq!(report.total_keys, 2);
        assert_eq!(report.leaves, 2);
        // Two keys that differ in one bit may trigger graduation
        // from Node2 to Node4, so we check for at least 1 internal node
        assert!(report.total_internal >= 1);
        assert!(report.depth >= 2);
    }

    #[test]
    fn graduation_two_leaves() {
        // Insert two keys that fill a Node2 with both leaves.
        // "a" and "b" diverge at bit 6, so Node2@6 with two leaves.
        // Both leaves are always placeable, so graduation to Node4@6 should happen.
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"a".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"b".to_vec(), 2).unwrap();
        // After graduation, should still be able to look up both keys
        assert_eq!(trie.get(b"a\0"), Some(i1));
        assert_eq!(trie.get(b"b\0"), Some(i2));
        let report = trie.structure_report();
        // Should have graduated from Node2 to Node4
        assert!(report.node4 >= 1, "expected at least 1 Node4, got {}", report.node4);
    }

    #[test]
    fn graduation_three_keys() {
        // Insert "a", "b", "c". After "a" and "b", graduation creates Node4.
        // Then "c" should still work.
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let i1 = trie.insert(b"a".to_vec(), 1).unwrap();
        let i2 = trie.insert(b"b".to_vec(), 2).unwrap();
        let i3 = trie.insert(b"c".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"a\0"), Some(i1));
        assert_eq!(trie.get(b"b\0"), Some(i2));
        assert_eq!(trie.get(b"c\0"), Some(i3));
    }

    #[test]
    fn graduation_debug_key_prefix() {
        // Debug: insert "key_000" through "key_009" and verify lookups
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..10 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        #[cfg(debug_assertions)]
        trie.dump();
        for i in 0..10 {
            let key = format!("key_{:03}\0", i);
            let result = trie.get(key.as_bytes());
            if result != Some(i) {
                eprintln!("FAIL: get({:?}) = {:?}, expected Some({})", key, result, i);
            }
        }
        for i in 0..10 {
            let key = format!("key_{:03}\0", i);
            let result = trie.get(key.as_bytes());
            assert_eq!(result, Some(i), "get({:?}) failed for i={}", key, i);
        }
    }

    #[test]
    fn structure_report_many_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        let report = trie.structure_report();
        assert_eq!(report.total_keys, 100);
        assert_eq!(report.leaves, 100);
        assert!(report.total_internal > 0);
        assert!(report.depth > 1);
        // Invariants
        assert_eq!(
            report.total_internal,
            report.node2 + report.node4 + report.node16
        );
    }

    #[test]
    fn structure_report_display() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        let report = trie.structure_report();
        let s = format!("{report}");
        assert!(s.contains("Keys:"));
        assert!(s.contains("Node2:"));
        assert!(s.contains("Depth:"));
    }

    #[test]
    fn aligned_graduation_creates_node4() {
        // Keys that diverge at bit 6 (even position) should allow Node2→Node4
        // graduation when both slots fill. "a" and "b" differ at bit 6 (0x61 vs 0x62).
        // In MSB ordering: bit 6 is even, so graduation is allowed.
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"a".to_vec(), 1).unwrap();
        trie.insert(b"b".to_vec(), 2).unwrap();
        let report = trie.structure_report();
        assert!(report.node4 >= 1, "expected Node4 from aligned graduation, got node4={}", report.node4);
        assert_eq!(trie.get(b"a\0"), Some(0));
        assert_eq!(trie.get(b"b\0"), Some(1));
    }

    #[test]
    fn aligned_graduation_stress_1000() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..1000u32 {
            let key = format!("key_{:05}", i);
            let result = trie.insert(key.into_bytes(), i as i32);
            assert!(result.is_ok(), "insert failed at i={}", i);
        }
        let report = trie.structure_report();
        println!("Stress: keys={}, node2={}, node4={}, node16={}",
            report.total_keys, report.node2, report.node4, report.node16);
        // All lookups must succeed
        for i in 0..1000u32 {
            let key = format!("key_{:05}\0", i);
            let result = trie.get(key.as_bytes());
            assert_eq!(result, Some(i as usize), "lookup failed at i={}", i);
        }
    }

    #[test]
    fn aligned_graduation_byte_boundary_keys() {
        // Keys that diverge at byte boundaries (bit positions 0, 8, 16, etc.)
        // are always aligned for all radix widths. Skip 0x00 (null byte rejected).
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for b in 1u8..=255 {
            trie.insert(vec![b], b as i32).unwrap();
        }
        assert_eq!(trie.len(), 255);
        let report = trie.structure_report();
        println!("Byte keys: node2={}, node4={}, node16={}",
            report.node2, report.node4, report.node16);
        // Verify all lookups
        for b in 1u8..=255 {
            let key = vec![b, 0];
            assert_eq!(trie.get(&key), Some(b as usize - 1), "lookup failed for byte {}", b);
        }
    }

    // -----------------------------------------------------------------------
    // Iterator tests
    // -----------------------------------------------------------------------

    #[test]
    fn iter_empty() {
        let trie: PolyTrie<i32> = PolyTrie::new();
        let mut it = trie.iter();
        assert!(it.next().is_none());
        assert!(it.prev().is_none());
        assert!(it.current().is_none());
    }

    #[test]
    fn iter_single_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"hello".to_vec(), 42).unwrap();
        let mut it = trie.iter();
        assert!(it.current().is_none()); // before first
        let (k, v) = it.next().unwrap();
        assert_eq!(k, b"hello");
        assert_eq!(*v, 42);
        assert!(it.next().is_none()); // exhausted
    }

    #[test]
    fn iter_forward() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
        for (i, key) in keys.iter().enumerate() {
            trie.insert(key.clone(), i as i32).unwrap();
        }
        let mut it = trie.iter();
        assert_eq!(it.next().unwrap().0, b"abc");
        assert_eq!(it.next().unwrap().0, b"abd");
        assert_eq!(it.next().unwrap().0, b"abz");
        assert!(it.next().is_none());
    }

    #[test]
    fn iter_backward() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
        for (i, key) in keys.iter().enumerate() {
            trie.insert(key.clone(), i as i32).unwrap();
        }
        let mut it = trie.iter_last();
        // current() should give the last key
        assert_eq!(it.current().unwrap().0, b"abz");
        assert_eq!(it.prev().unwrap().0, b"abd");
        assert_eq!(it.prev().unwrap().0, b"abc");
        assert!(it.prev().is_none());
    }

    #[test]
    fn iter_backward_full() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
        for (i, key) in keys.iter().enumerate() {
            trie.insert(key.clone(), i as i32).unwrap();
        }
        // Start at last, walk backward without calling current()
        let mut it = trie.iter_last();
        let mut collected: Vec<Vec<u8>> = Vec::new();
        // First prev() gives second-to-last key
        while let Some((k, _)) = it.prev() {
            collected.push(k.to_vec());
        }
        // We should get 2 keys (abd, abc) since current() wasn't called first
        // Wait - iter_last positions at the LAST key, so prev() starts from 2nd-to-last
        // If we want ALL keys, we need current() first
        assert_eq!(collected.len(), 2); // abd, abc (without calling current() first)
        assert_eq!(collected[0], b"abd");
        assert_eq!(collected[1], b"abc");
    }

    #[test]
    fn iter_forward_backward_interleaved() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for c in b'a'..=b'f' {
            trie.insert(vec![c], c as i32).unwrap();
        }
        // Test forward iteration
        let mut it = trie.iter();
        assert_eq!(it.next().unwrap().0, b"a");
        assert_eq!(it.next().unwrap().0, b"b");
        assert_eq!(it.next().unwrap().0, b"c");
        // Now backward
        assert_eq!(it.prev().unwrap().0, b"b");
        assert_eq!(it.prev().unwrap().0, b"a");
        // Can't go further back
        assert!(it.prev().is_none());
    }

    #[test]
    fn iter_seek_exact() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abz".to_vec(), 3).unwrap();
        let mut it = trie.iter();
        let (k, v) = it.seek(b"abd\0").unwrap();
        assert_eq!(k, b"abd");
        assert_eq!(*v, 2);
    }

    #[test]
    fn iter_seek_between() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();
        trie.insert(b"abz".to_vec(), 3).unwrap();
        let mut it = trie.iter();
        // "abe" is between "abd" and "abz"
        let (k, _) = it.seek(b"abe\0").unwrap();
        assert_eq!(k, b"abz");
    }

    #[test]
    fn iter_seek_prefix_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abcd".to_vec(), 2).unwrap();
        // Seek to "abc" should find "abc" exactly
        let mut it = trie.iter();
        let (k, v) = it.seek(b"abc\0").unwrap();
        assert_eq!(k, b"abc");
        assert_eq!(*v, 1);
    }

    #[test]
    fn iter_seek_past_end() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        trie.insert(b"abd".to_vec(), 2).unwrap();

        // Forward iteration should return both keys
        let mut it = trie.iter();
        assert_eq!(it.next().unwrap().0, b"abc");
        assert_eq!(it.next().unwrap().0, b"abd");
        assert!(it.next().is_none());

        // Seek past all keys should return None
        let mut it = trie.iter();
        assert!(it.seek(b"zzz\0").is_none());

        // Seek between existing keys should find the next one
        let mut it = trie.iter();
        let (k, _) = it.seek(b"abcd\0").unwrap();
        assert_eq!(k, b"abd");

        // Seek to exact key should find it
        let mut it = trie.iter();
        let (k, _) = it.seek(b"abc\0").unwrap();
        assert_eq!(k, b"abc");
    }

    #[test]
    fn iter_seek_before_all() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"def".to_vec(), 1).unwrap();
        trie.insert(b"xyz".to_vec(), 2).unwrap();
        let mut it = trie.iter();
        let (k, _) = it.seek(b"abc\0").unwrap();
        assert_eq!(k, b"def");
    }

    #[test]
    fn iter_stress_forward() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let n = 200;
        for i in 0..n {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        let mut it = trie.iter();
        let mut keys: Vec<Vec<u8>> = Vec::new();
        while let Some((k, _)) = it.next() {
            keys.push(k.to_vec());
        }
        assert_eq!(keys.len(), n);
        for i in 1..keys.len() {
            assert!(keys[i] > keys[i - 1], "not in sorted order at index {}", i);
        }
    }

    #[test]
    fn iter_stress_backward() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        let n = 200;
        for i in 0..n {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        let mut it = trie.iter_last();
        let mut keys: Vec<Vec<u8>> = Vec::new();
        loop {
            match it.current() {
                Some((k, _)) => keys.push(k.to_vec()),
                None => break,
            }
            if it.prev().is_none() {
                break;
            }
        }
        assert_eq!(keys.len(), n);
        for i in 1..keys.len() {
            assert!(keys[i] < keys[i - 1], "not in reverse order at index {}", i);
        }
    }

    #[test]
    fn iter_with_graduation() {
        // Insert enough keys to trigger graduation through Node2 → Node4 → Node16
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100u32 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        let report = trie.structure_report();
        // Should have some graduated nodes
        assert!(report.node4 + report.node16 > 0, "expected some graduated nodes");

        // Verify forward iteration
        let mut it = trie.iter();
        let mut forward_keys: Vec<Vec<u8>> = Vec::new();
        while let Some((k, _)) = it.next() {
            forward_keys.push(k.to_vec());
        }
        assert_eq!(forward_keys.len(), 100);
        for i in 1..forward_keys.len() {
            assert!(forward_keys[i] > forward_keys[i - 1]);
        }

        // Verify backward iteration
        let mut it = trie.iter_last();
        let mut backward_keys: Vec<Vec<u8>> = Vec::new();
        loop {
            match it.current() {
                Some((k, _)) => backward_keys.push(k.to_vec()),
                None => break,
            }
            if it.prev().is_none() {
                break;
            }
        }
        assert_eq!(backward_keys.len(), 100);
        for i in 1..backward_keys.len() {
            assert!(backward_keys[i] < backward_keys[i - 1]);
        }
    }

    #[test]
    fn iter_byte_boundary_keys() {
        // 255 single-byte keys (0x01..=0xFF) — creates wider node types
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for b in 1u8..=255 {
            trie.insert(vec![b], b as i32).unwrap();
        }
        // Forward iteration — collect and verify order
        let mut it = trie.iter();
        let mut keys: Vec<u8> = Vec::new();
        while let Some((k, _)) = it.next() {
            assert_eq!(k.len(), 1, "single-byte key expected, got {:?}", k);
            keys.push(k[0]);
        }
        assert_eq!(keys.len(), 255);
        for i in 1..keys.len() {
            assert!(keys[i] > keys[i - 1], "not in order: {} <= {}", keys[i], keys[i - 1]);
        }

        // Backward iteration
        let mut it = trie.iter_last();
        keys.clear();
        loop {
            match it.current() {
                Some((k, _)) => { assert_eq!(k.len(), 1); keys.push(k[0]); }
                None => break,
            }
            if it.prev().is_none() { break; }
        }
        assert_eq!(keys.len(), 255);
        for i in 1..keys.len() {
            assert!(keys[i] < keys[i - 1], "not in reverse order");
        }
    }

    #[test]
    fn iter_seek_stress() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100u32 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        // Seek to each exact key (null-terminated)
        for i in 0..100u32 {
            let key = format!("key_{:05}\0", i);
            let mut it = trie.iter();
            let (k, v) = it.seek(key.as_bytes()).unwrap();
            assert_eq!(k, &format!("key_{:05}", i).into_bytes()[..]);
            assert_eq!(*v, i as i32);
        }
        // Seek between keys
        let mut it = trie.iter();
        let (k, _) = it.seek(b"key_00050\0").unwrap();
        assert_eq!(k, b"key_00050");

        let mut it = trie.iter();
        let (k, _) = it.seek(b"key_00049\x01\0").unwrap();
        // Between key_00049 and key_00050: should land on key_00050
        assert!(k >= b"key_00050", "expected key >= key_00050, got {:?}", k);
    }

    // -----------------------------------------------------------------------
    // Optimize tests
    // -----------------------------------------------------------------------

    #[test]
    fn optimize_empty() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.optimize();
        assert!(trie.is_empty());
    }

    #[test]
    fn optimize_single_key() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        trie.insert(b"hello".to_vec(), 42).unwrap();
        trie.optimize();
        assert_eq!(trie.get(b"hello\0"), Some(0));
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn optimize_preserves_lookups() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        trie.optimize();
        for i in 0..100 {
            let key = format!("key_{:03}\0", i);
            assert_eq!(trie.get(key.as_bytes()), Some(i),
                "lookup failed after optimize for i={}", i);
        }
        assert_eq!(trie.len(), 100);
    }

    #[test]
    fn optimize_preserves_iteration() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100u32 {
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
    fn optimize_compacts_arena() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..50 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        let before_occupied = trie.arena.len();
        let before_capacity = trie.arena.capacity();
        trie.optimize();
        let after_occupied = trie.arena.len();
        let after_capacity = trie.arena.capacity();
        // Occupied slots should be the same (same number of live nodes)
        assert_eq!(after_occupied, before_occupied);
        // Capacity should equal occupied (no freed gaps)
        assert_eq!(after_capacity, after_occupied,
            "arena not compact after optimize: capacity={} occupied={}",
            after_capacity, after_occupied);
        // Capacity should be <= before (freed slots from graduation are reclaimed)
        assert!(after_capacity <= before_capacity);
    }

    #[test]
    fn optimize_byte_boundary_keys() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for b in 1u8..=255 {
            trie.insert(vec![b], b as i32).unwrap();
        }
        trie.optimize();
        for b in 1u8..=255 {
            let key = vec![b, 0];
            assert_eq!(trie.get(&key), Some(b as usize - 1),
                "lookup failed after optimize for byte {}", b);
        }
    }

    #[test]
    fn optimize_seek_preserved() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..50u32 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        trie.optimize();
        let mut it = trie.iter();
        let (k, v) = it.seek(b"key_00025\0").unwrap();
        assert_eq!(k, b"key_00025");
        assert_eq!(*v, 25);
    }

    #[test]
    fn optimize_idempotent() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        trie.optimize();
        let cap1 = trie.arena.capacity();
        trie.optimize();
        let cap2 = trie.arena.capacity();
        assert_eq!(cap1, cap2, "second optimize changed arena size");
        for i in 0..100 {
            let key = format!("key_{:03}\0", i);
            assert_eq!(trie.get(key.as_bytes()), Some(i));
        }
    }

    #[test]
    fn optimize_stress_1000() {
        let mut trie: PolyTrie<i32> = PolyTrie::new();
        for i in 0..1000u32 {
            let key = format!("key_{:05}", i);
            trie.insert(key.into_bytes(), i as i32).unwrap();
        }
        trie.optimize();
        for i in 0..1000u32 {
            let key = format!("key_{:05}\0", i);
            assert_eq!(trie.get(key.as_bytes()), Some(i as usize),
                "lookup failed after optimize at i={}", i);
        }
    }
}