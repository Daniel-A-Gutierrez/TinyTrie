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
//! A dummy entry at `index[0] = (0, 0)` points at `buf[0]` (empty key).
//! Real keys start at index 1. This allows 0 to be used as a sentinel for
//! "empty" in `children[]` slots. Keys are stored contiguously in `buf` with
//! a side index `(offset, len)`, saving ~24 bytes/key vs `Vec<Vec<u8>>`.
//!
//! # Leaf and Offset Fields
//!
//! `Node.leaf` is a key index (into `index[]`), used for retrieval — it
//! gives the value index via `values[leaf - 1]` and the key via `index[leaf]`.
//! `Node.offset` is a direct offset into `buf`, used for insertion divergence
//! comparison and the `get()` terminal fast path. For terminal nodes, the key
//! is `buf[offset..offset + prefix_len/2]` — no `index` lookup needed.

use crate::TinyTrieMap;
use std::{fmt, simd::{LaneCount, Simd, SupportedLaneCount, cmp::SimdPartialEq}};

// ---------------------------------------------------------------------------
// TrieIndex trait
// ---------------------------------------------------------------------------

/// Trait for types used as arena/key indices and prefix lengths in NibbleTrie.
///
/// Implemented for `u16`, `u32`, and `u64`. The type parameter `PTR` (pointer
/// type) controls the width of `children`, `leaf`, and arena indices. The type
/// parameter `LEN` (length type) controls the width of `prefix_len` and key
/// lengths in the index.
pub trait TrieIndex: Copy + Clone + Default + PartialEq + Eq + fmt::Debug + 'static {
    /// Convert to `usize` for indexing.
    fn as_usize(self) -> usize;
    /// Maximum representable value (e.g. `u16::MAX` for u16).
    fn max_value() -> usize;
    /// Zero value, used for empty slots and initial values.
    fn zero() -> Self;
    /// Convert from `usize`. May panic or truncate on overflow in debug builds.
    fn from_usize(n: usize) -> Self;
    /// Compute a 16-bit occupancy mask from a 16-slot children array.
    /// Bit N is set if `children[N] != 0`.
    fn children_mask(children: &[Self; 16]) -> u16;
}

impl TrieIndex for u8 {
    #[inline] fn as_usize(self) -> usize { self as usize }
    #[inline] fn max_value() -> usize { u8::MAX as usize }
    #[inline] fn zero() -> Self { 0 }
    #[inline] fn from_usize(n: usize) -> Self {
        debug_assert!(n <= u8::MAX as usize, "u8 overflow: {n}");
        n as u8
    }
    #[inline] fn children_mask(children: &[Self; 16]) -> u16 {
        crate::simd::children_mask_u8(children)
    }
}

impl TrieIndex for u16 {
    #[inline] fn as_usize(self) -> usize { self as usize }
    #[inline] fn max_value() -> usize { u16::MAX as usize }
    #[inline] fn zero() -> Self { 0 }
    #[inline] fn from_usize(n: usize) -> Self {
        debug_assert!(n <= u16::MAX as usize, "u16 overflow: {n}");
        n as u16
    }
    #[inline] fn children_mask(children: &[Self; 16]) -> u16 {
        crate::simd::children_mask_u16(children)
    }
}

impl TrieIndex for u32 {
    #[inline] fn as_usize(self) -> usize { self as usize }
    #[inline] fn max_value() -> usize { u32::MAX as usize }
    #[inline] fn zero() -> Self { 0 }
    #[inline] fn from_usize(n: usize) -> Self {
        debug_assert!(n <= u32::MAX as usize, "u32 overflow: {n}");
        n as u32
    }
    #[inline] fn children_mask(children: &[Self; 16]) -> u16 {
        crate::simd::children_mask(children)
    }
}

impl TrieIndex for u64 {
    #[inline] fn as_usize(self) -> usize { self as usize }
    #[inline] fn max_value() -> usize { u64::MAX as usize }
    #[inline] fn zero() -> Self { 0 }
    #[inline] fn from_usize(n: usize) -> Self { n as u64 }
    #[inline] fn children_mask(children: &[Self; 16]) -> u16 {
        crate::simd::children_mask_u64(children)
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Bit 63 of `Node.offset` stores the terminal flag.
const TERMINAL_BIT: u64 = 1u64 << 63;

/// A single node in the nibble trie arena.
///
/// Generic over `PTR` (pointer/index type for children and arena references)
/// and `LEN` (length type for prefix lengths and key lengths).
///
/// Layout with PTR=u32, LEN=u16 (default): 80 bytes (backward-compatible).
/// Layout with PTR=u16, LEN=u16 (compact):  48 bytes (fits a 64-byte cache line).
///
/// - `children`: 16 slots indexed by nibble value. `0` = empty slot.
///   For internal nodes, the value is an arena index (≥ 1).
///   For leaves (when `leaf_mask` bit is set), the value is a key index (≥ 1,
///   since `index[0]` is the dummy).
/// - `prefix_len`: absolute nibble position of this node's discriminating
///   nibble. During lookup, use `key_nibble_at(key, prefix_len)` directly —
///   no accumulation needed.
/// - `leaf_mask`: bit N set → `children[N]` holds a leaf key index.
/// - `leaf`: key index of a reference key. When terminal, this is the node's
///   own key index. When not terminal, it's a descendant leaf key index.
///   Used for retrieval: value lookup via `values[leaf - 1]` and key via
///   `index[leaf]`.
/// - `offset`: direct offset into `buf` for this node's reference key,
///   with the terminal flag packed into bit 63. For terminal nodes, the key
///   is `buf[raw_offset..raw_offset + prefix_len/2]`, avoiding an `index`
///   lookup entirely. For non-terminal nodes, used during insertion to get
///   the reference key for divergence comparison.
#[derive(Copy, Clone)]
pub struct Node<PTR: TrieIndex, LEN: TrieIndex> {
    pub children: [PTR; 16],
    pub prefix_len: LEN,
    pub leaf_mask: u16,
    pub leaf: PTR,
    pub offset: u64,  // bit 63 = terminal, bits 0-62 = raw buf offset
}

impl<PTR: TrieIndex, LEN: TrieIndex> Node<PTR, LEN> {
    fn new() -> Self {
        Node {
            children: [PTR::zero(); 16],
            prefix_len: LEN::zero(),
            leaf_mask: 0,
            leaf: PTR::zero(),
            offset: 0,
        }
    }

    #[inline]
    pub fn is_terminal(&self) -> bool {
        (self.offset & TERMINAL_BIT) != 0
    }

    #[inline]
    fn set_terminal(&mut self, val: bool) {
        if val {
            self.offset |= TERMINAL_BIT;
        } else {
            self.offset &= !TERMINAL_BIT;
        }
    }

    #[inline]
    fn raw_offset(&self) -> u64 {
        self.offset & !TERMINAL_BIT
    }

    #[inline]
    fn set_raw_offset(&mut self, off: u64) {
        self.offset = (self.offset & TERMINAL_BIT) | (off & !TERMINAL_BIT);
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

    /// Store a leaf key index at `nib`. Key index must be ≥ 1
    /// (index[0] is the dummy entry).
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, key_index: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(key_index != PTR::zero(), "key index 0 is the dummy");
        self.set_leaf(nib);
        self.children[nib] = key_index;
    }

    /// Store an arena index at `nib` (internal node reference).
    /// Arena index must be ≥ 1 (root at index 0 is never a child of another node).
    #[inline]
    fn set_internal_child(&mut self, nib: usize, arena_index: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(arena_index != PTR::zero());
        self.clear_leaf(nib);
        self.children[nib] = arena_index;
    }

    /// Decode a leaf child at `nib` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize) -> Option<PTR> {
        debug_assert!(nib < 16);
        if self.is_leaf(nib) && self.children[nib] != PTR::zero() {
            Some(self.children[nib])
        } else {
            None
        }
    }

    /// Compute a 16-bit mask where bit N is set if `children[N] != 0`.
    /// Uses SIMD (for u16/u32/u64) to evaluate all 16 slots in parallel.
    #[inline]
    pub fn children_mask(&self) -> u16 {
        PTR::children_mask(&self.children)
    }
}

impl<PTR: TrieIndex, LEN: TrieIndex> fmt::Debug for Node<PTR, LEN> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active: Vec<(usize, &str, PTR)> = (0..16)
            .filter(|&n| self.children[n] != PTR::zero())
            .map(|n| {
                let tag = if self.is_leaf(n) { "L" } else { "I" };
                (n, tag, self.children[n])
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("0x{:04x}", self.leaf_mask))
            .field("leaf", &self.leaf)
            .field("raw_offset", &self.raw_offset())
            .field("terminal", &self.is_terminal())
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// NibbleTrie
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NibbleTrie<T, PTR: TrieIndex = u32, LEN: TrieIndex = u16> {
    pub arena: Vec<Node<PTR, LEN>>,
    pub buf: Vec<u8>,                // all keys concatenated (no null terminators)
    pub index: Vec<(usize, LEN)>,    // (offset into buf, len) per key — offset is usize, len is compact
    pub values: Vec<T>,              // values[i] ↔ index[i]
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

/// SIMD-accelerated byte equality check. Returns `true` if both slices have
/// the same length and identical content. Uses 16-byte lanes for the bulk
/// of the comparison, with a scalar tail for the remainder.
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

/// Unchecked version of `simd_eq` — skips the length check and uses unchecked
/// indexing throughout. The caller must guarantee that `a` and `b` have the
/// same length.
///
/// # Safety
/// `a` and `b` must have the same length. All SIMD and scalar accesses must
/// be in bounds (guaranteed if lengths are equal and non-empty).
#[inline]
unsafe fn simd_eq_unchecked(a: &[u8], b: &[u8]) -> bool {
    debug_assert_eq!(a.len(), b.len());
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

/// Unchecked version of `key_nibble_at`.
///
/// # Safety
/// `idx / 2` must be < `key.len()` (i.e., the nibble index must be in bounds).
#[inline]
unsafe fn key_nibble_at_unchecked(key: &[u8], idx: usize) -> u8 {
    let byte_idx = idx / 2;
    debug_assert!(byte_idx < key.len(), "nibble {idx} out of bounds for key len {}", key.len());
    if idx % 2 == 0 {
        unsafe { *key.get_unchecked(byte_idx) >> 4 }
    } else {
        unsafe { *key.get_unchecked(byte_idx) & 0x0F }
    }
}

#[inline]
fn nibble_count(key: &[u8]) -> usize {
    key.len() * 2
}

// ---------------------------------------------------------------------------
// NibbleTrie implementation
// ---------------------------------------------------------------------------

impl<T, PTR: TrieIndex, LEN: TrieIndex> NibbleTrie<T, PTR, LEN> {
    /// Return the key slice for `key_index`.
    #[inline]
    fn key_slice(&self, key_index: PTR) -> &[u8] {
        let (off, len) = self.index[key_index.as_usize()];
        &self.buf[off..off + len.as_usize()]
    }

    pub fn new() -> Self {
        NibbleTrie {
            arena: Vec::new(),
            buf: vec![0],           // buf[0] = dummy (unused byte)
            index: vec![(0, LEN::zero())],   // index[0] = dummy entry (offset 0, len 0)
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
        let mut node_idx: PTR = PTR::zero();
        let max_nib = key.len() * 2;
        loop {
            let node = &self.arena[node_idx.as_usize()];
            // Key nibbles exhausted — check if this node is terminal
            if node.prefix_len.as_usize() >= max_nib {
                if node.is_terminal() {
                    let key_len = node.prefix_len.as_usize() / 2;
                    let off = node.raw_offset() as usize;
                    let key_in_buf = &self.buf[off..off + key_len];
                    if simd_eq(key_in_buf, key) {
                        return Some(node.leaf.as_usize());
                    }
                }
                return None;
            }
            let nib = key_nibble_at(key, node.prefix_len.as_usize()) as usize;
            let slot = node.children[nib];
            if slot == PTR::zero() {
                return None;
            }
            if node.is_leaf(nib) {
                let key_index = slot;
                debug_assert!(key_index != PTR::zero());
                return if simd_eq(self.key_slice(key_index), key) {
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
    /// Follows the nibble path through the trie structure and returns the
    /// key index directly once a terminal node or leaf is reached, **without
    /// verifying key equality**. This is the key difference from `get()`:
    /// the final SIMD key comparison is skipped entirely, which is the
    /// dominant cost of a normal lookup.
    ///
    /// # Safety
    ///
    /// The key **must** have been inserted into this trie. If the key is not
    /// present, the result is unspecified (may return a wrong index or `None`).
    /// All child/leaf indices encountered during traversal must be valid arena
    /// or index entries.
    pub unsafe fn get_unchecked(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut node_idx: PTR = PTR::zero();
        let max_nib = key.len() * 2;
        loop {
            // SAFETY: node_idx is always a valid arena index (root=0,
            // children come from prior arena[node].children which are valid).
            let node = unsafe { self.arena.get_unchecked(node_idx.as_usize()) };
            let prefix_len = node.prefix_len.as_usize();
            // Key nibbles exhausted — under the "key in set" assumption,
            // this node must be terminal. Return the key index directly.
            if prefix_len >= max_nib {
                debug_assert!(node.is_terminal(), "get_unchecked: key not in set");
                return Some(node.leaf.as_usize());
            }
            // SAFETY: prefix_len < max_nib implies byte_idx < key.len().
            let nib = unsafe { key_nibble_at_unchecked(key, prefix_len) } as usize;
            // SAFETY: nib < 16 always; children has 16 slots.
            let slot = unsafe { *node.children.get_unchecked(nib) };
            if slot == PTR::zero() {
                return None;
            }
            if node.is_leaf(nib) {
                // Under the "key in set" assumption, this leaf is the
                // correct one — no key comparison needed.
                return Some(slot.as_usize());
            }
            node_idx = slot;
        }
    }

    /// Unchecked version of `key_slice` — skips bounds checks on index and buf.
    ///
    /// # Safety
    /// `key_index` must be a valid index into `self.index`, and the offset/length
    /// stored there must describe a valid range within `self.buf`.
    #[inline]
    unsafe fn key_slice_unchecked(&self, key_index: PTR) -> &[u8] {
        let (off, len) = unsafe { *self.index.get_unchecked(key_index.as_usize()) };
        unsafe { self.buf.get_unchecked(off..off + len.as_usize()) }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1]) // values[0] corresponds to keys[1]
    }

    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        // Overflow checks: arena/key indices must fit in PTR.
        // Each insert adds at most 1 arena node, so arena.len() must stay <= max_value.
        // Key indices start at 1 (index[0] is dummy), so index.len() must stay <= max_value.
        // Key nibble count must fit in LEN (for prefix_len). Buf offsets are usize/u64,
        // so total buffer size is only limited by available memory.
        if self.arena.len() > PTR::max_value() {
            return Err(());
        }
        if self.index.len() > PTR::max_value() {
            return Err(());
        }
        if key.len() * 2 > LEN::max_value() {
            return Err(());
        }

        // Real key index: index[0] is the dummy, so real keys start at 1
        let new_index = PTR::from_usize(self.index.len());
        let key_len = LEN::from_usize(key.len());
        let offset = self.buf.len() as u64;
        self.buf.extend_from_slice(&key);
        self.index.push((offset as usize, key_len));
        self.values.push(value);

        let max_nib = key.len() * 2;

        if self.arena.is_empty() {
            if max_nib == 0 {
                // Empty key — root node itself is terminal
                let mut root = Node::new();
                root.set_terminal(true);
                root.leaf = new_index;
                root.set_raw_offset(offset);
                self.arena.push(root);
                return Ok(new_index.as_usize());
            }
            let first_nib = key_nibble_at(&key, 0) as usize;
            let mut root = Node::new();
            root.set_leaf_child(first_nib, new_index);
            root.leaf = new_index;
            root.set_raw_offset(offset);
            self.arena.push(root);
            return Ok(new_index.as_usize());
        }

        let mut node_idx: PTR = PTR::zero();
        // Nibbles 0..confirmed-1 are known to match between new_key and any
        // key in the current subtree. Skipping them avoids re-scanning the
        // shared prefix at every level of descent.
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[node_idx.as_usize()];
            // Use node.raw_offset() for direct buf access — no index lookup needed
            let (_, ref_len) = self.index[node.leaf.as_usize()];
            let off = node.raw_offset() as usize;
            let ref_key = &self.buf[off..off + ref_len.as_usize()];
            let prefix_len = node.prefix_len.as_usize();

            match simd_find_divergence::<8>(&key, ref_key, confirmed) {
                DivergeResult::Duplicate => {
                    // Roll back the key we just appended
                    let (off, _len) = self.index.pop().unwrap();
                    self.buf.truncate(off);
                    let _ = self.values.pop();
                    return Err(());
                }
                DivergeResult::At(diverge) if diverge < prefix_len => {
                    // Divergence before the discriminating nibble — split this node
                    let new_nib = key_nibble_at(&key, diverge) as usize;
                    let ref_nib = key_nibble_at(ref_key, diverge) as usize;

                    let mut new_parent = Node::new();
                    new_parent.prefix_len = LEN::from_usize(diverge);

                    if diverge >= max_nib {
                        // New key ends at the split point — terminal
                        new_parent.set_terminal(true);
                        new_parent.leaf = new_index;
                        new_parent.set_raw_offset(offset);
                    } else {
                        new_parent.set_leaf_child(new_nib, new_index);
                        new_parent.leaf = new_index;
                        new_parent.set_raw_offset(offset);
                    }

                    let old_node = std::mem::replace(
                        &mut self.arena[node_idx.as_usize()],
                        new_parent,
                    );
                    let old_idx = PTR::from_usize(self.arena.len());
                    self.arena.push(old_node);

                    self.arena[node_idx.as_usize()].set_internal_child(ref_nib, old_idx);
                    self.sort_internal_children(node_idx);

                    return Ok(new_index.as_usize());
                }
                DivergeResult::At(_) => {
                    // Divergence at or after prefix_len — follow the child.
                    // But first check if the new key is a prefix that ends here.
                    if max_nib <= prefix_len {
                        // Key nibbles exhausted at this node — mark terminal
                        self.arena[node_idx.as_usize()].set_terminal(true);
                        self.arena[node_idx.as_usize()].leaf = new_index;
                        self.arena[node_idx.as_usize()].set_raw_offset(offset);
                        return Ok(new_index.as_usize());
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(&key, prefix_len) as usize;
                    let slot = node.children[nib];

                    if slot == PTR::zero() {
                        // Empty slot — new key diverges here
                        self.arena[node_idx.as_usize()].set_leaf_child(nib, new_index);
                        return Ok(new_index.as_usize());
                    }

                    if node.is_leaf(nib) {
                        let existing_key_index = slot;
                        let (existing_offset, existing_len) = self.index[existing_key_index.as_usize()];
                        let existing_key = &self.buf[existing_offset..existing_offset + existing_len.as_usize()];

                        match simd_find_divergence::<8>(&key, existing_key, confirmed) {
                            DivergeResult::Duplicate => {
                                let (off, _len) = self.index.pop().unwrap();
                                self.buf.truncate(off);
                                let _ = self.values.pop();
                                return Err(());
                            }
                            DivergeResult::At(d) => {
                                let mut split_node = Node::new();
                                split_node.prefix_len = LEN::from_usize(d);

                                if d >= max_nib {
                                    // New key ends at the split point — terminal
                                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                                    split_node.set_terminal(true);
                                    split_node.leaf = new_index;
                                    split_node.set_raw_offset(offset);
                                    split_node.set_leaf_child(exist_nib, existing_key_index);
                                } else if d >= existing_key.len() * 2 {
                                    // Existing key ends at the split point — terminal
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    split_node.set_terminal(true);
                                    split_node.leaf = existing_key_index;
                                    split_node.set_raw_offset(existing_offset as u64);
                                    split_node.set_leaf_child(new_nib, new_index);
                                } else {
                                    // Neither key ends at the split point — they diverge here
                                    let new_nib = key_nibble_at(&key, d) as usize;
                                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                                    debug_assert_ne!(new_nib, exist_nib);
                                    split_node.set_leaf_child(new_nib, new_index);
                                    split_node.set_leaf_child(exist_nib, existing_key_index);
                                    split_node.leaf = existing_key_index;
                                    split_node.set_raw_offset(existing_offset as u64);
                                }

                                let split_idx = PTR::from_usize(self.arena.len());
                                self.arena.push(split_node);
                                self.arena[node_idx.as_usize()].set_internal_child(nib, split_idx);
                                self.sort_internal_children(node_idx);

                                return Ok(new_index.as_usize());
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

    pub fn iter(&self) -> NibbleIter<'_, T, PTR, LEN> {
        NibbleIter::new(self)
    }

    pub fn iter_last(&self) -> NibbleIter<'_, T, PTR, LEN> {
        NibbleIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<Vec<u8>>, Vec<T>) {
        // Skip index[0] (dummy)
        let buf = self.buf;
        let keys: Vec<Vec<u8>> = self.index.into_iter().skip(1).map(|(off, len)| {
            buf[off..off + len.as_usize()].to_vec()
        }).collect();
        (keys, self.values)
    }

    /// Swap two arena nodes and fix all parent references.
    /// After this, what was at index `a` is now at index `b` and vice versa.
    fn swap_arena(&mut self, a: PTR, b: PTR) {
        if a == b {
            return;
        }
        self.arena.swap(a.as_usize(), b.as_usize());
        // Fix references in every node that pointed at a or b
        for node in &mut self.arena {
            for nib in 0..16 {
                if node.children[nib] != PTR::zero() && !node.is_leaf(nib) {
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
    fn sort_internal_children(&mut self, node_idx: PTR) {
        // Collect internal children in nibble order: (nib, arena_idx)
        let mut internals: [u8; 16] = [0; 16];  // nibble values
        let mut arena_ids: [PTR; 16] = [PTR::zero(); 16]; // corresponding arena indices
        let mut count = 0usize;
        for nib in 0u8..16 {
            if self.arena[node_idx.as_usize()].children[nib as usize] != PTR::zero()
                && !self.arena[node_idx.as_usize()].is_leaf(nib as usize)
            {
                internals[count] = nib;
                arena_ids[count] = self.arena[node_idx.as_usize()].children[nib as usize];
                count += 1;
            }
        }
        if count <= 1 {
            return;
        }
        // Find where the new node (highest arena index) sits in nibble order
        let max_arena_idx = (0..count).fold(PTR::zero(), |m, i| {
            if arena_ids[i].as_usize() > m.as_usize() { arena_ids[i] } else { m }
        });
        let insert_pos = (0..count).find(|&i| arena_ids[i] == max_arena_idx).unwrap();
        // Rotate: swap arena nodes so that insert_pos moves to the end
        for i in insert_pos..count - 1 {
            self.swap_arena(arena_ids[i], arena_ids[i + 1]);
            // After swap, update our tracking
            let tmp = arena_ids[i];
            arena_ids[i] = arena_ids[i + 1];
            arena_ids[i + 1] = tmp;
        }
    }

    // -----------------------------------------------------------------------
    // Optimize (DFS key-sorted buf rewrite)
    // -----------------------------------------------------------------------

    /// Rewrite `buf` so that keys appear in sorted order, with contiguous
    /// layout for sequential access during iteration.
    ///
    /// After `optimize()`, a forward iteration hits `buf` in ascending
    /// memory order — sequential, prefetcher-friendly access.
    ///
    /// No-op for empty tries.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        // Pre-allocate at exact size. Writing into a pre-sized slice avoids
        // per-key Vec::extend_from_slice overhead (capacity checks, memcpy).
        let mut new_buf = vec![0u8; self.buf.len()];
        let mut cursor: u64 = 1; // position 0 is the dummy byte

        self.walk_optimize(PTR::zero(), &mut new_buf, &mut cursor);

        new_buf.truncate(cursor as usize);
        self.buf = new_buf;
    }

    /// Recursive DFS walk that copies keys into the pre-allocated `new_buf`
    /// slice in sorted order and updates `index` offsets and `node.offset`
    /// fields to point at the new locations.
    ///
    /// Visit order: terminal key first, then children 0–15 (leaf children
    /// inline, internal children via recursion). This yields keys in sorted
    /// order within each subtree.
    fn walk_optimize(
        &mut self,
        node_idx: PTR,
        new_buf: &mut [u8],
        cursor: &mut u64,
    ) {
        let node: Node<PTR, LEN> = self.arena[node_idx.as_usize()]; // copy to avoid borrow conflicts

        if node.is_terminal() {
            let ki = node.leaf;
            let (old_off, len) = self.index[ki.as_usize()];
            let start = *cursor as usize;
            new_buf[start..start + len.as_usize()].copy_from_slice(
                &self.buf[old_off..old_off + len.as_usize()]
            );
            self.index[ki.as_usize()].0 = *cursor as usize;
            *cursor += len.as_usize() as u64;
        }

        for nib in 0..16 {
            if node.children[nib] == PTR::zero() {
                continue;
            }
            if node.is_leaf(nib) {
                let ki = node.children[nib];
                let (old_off, len) = self.index[ki.as_usize()];
                let start = *cursor as usize;
                new_buf[start..start + len.as_usize()].copy_from_slice(
                    &self.buf[old_off..old_off + len.as_usize()]
                );
                self.index[ki.as_usize()].0 = *cursor as usize;
                *cursor += len.as_usize() as u64;
            } else {
                self.walk_optimize(node.children[nib], new_buf, cursor);
            }
        }

        // All descendant keys have been written and index offsets updated.
        // Now set this node's offset from its leaf's index entry.
        if node.leaf != PTR::zero() {
            let new_off = self.index[node.leaf.as_usize()].0;
            self.arena[node_idx.as_usize()].set_raw_offset(new_off as u64);
        }
    }

    // -----------------------------------------------------------------------
    // Capacity and promotion
    // -----------------------------------------------------------------------

    /// Returns `true` if the arena or key index is approaching the PTR type's
    /// capacity and the trie should be promoted to a wider PTR type before the
    /// next insert.
    ///
    /// DynNibbleTrie calls this before each insert and promotes automatically.
    pub fn near_capacity(&self) -> bool {
        self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value()
    }

    /// Consume this trie and produce a new one with a wider PTR type.
    /// Arena nodes are converted 1:1 (children/leaf fields widen). `buf`,
    /// `index`, and `values` transfer unchanged (they don't depend on PTR).
    ///
    /// Preserves all structural invariants: sorted internal children, terminal
    /// flags, leaf masks, and key/value associations.
    pub fn promote<NewPTR: TrieIndex>(self) -> NibbleTrie<T, NewPTR, LEN> {
        let arena: Vec<Node<NewPTR, LEN>> = self.arena.into_iter().map(|node| {
            let mut children = [NewPTR::zero(); 16];
            for i in 0..16 {
                children[i] = NewPTR::from_usize(node.children[i].as_usize());
            }
            Node {
                children,
                prefix_len: node.prefix_len,
                leaf_mask: node.leaf_mask,
                leaf: NewPTR::from_usize(node.leaf.as_usize()),
                offset: node.offset, // u64 — packed terminal bit transfers directly
            }
        }).collect();
        NibbleTrie {
            arena,
            buf: self.buf,
            index: self.index,  // (usize, LEN) — independent of PTR
            values: self.values,
        }
    }

    /// Consume this trie and produce a new one with a narrower PTR type.
    /// Returns `Err(self)` if the arena or key index doesn't fit in the
    /// narrower type.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<NibbleTrie<T, NewPTR, LEN>, Self> {
        // Check capacity before converting — all indices must fit in NewPTR.
        if self.arena.len() > NewPTR::max_value() || self.index.len() > NewPTR::max_value() {
            return Err(self);
        }
        let arena: Vec<Node<NewPTR, LEN>> = self.arena.into_iter().map(|node| {
            let mut children = [NewPTR::zero(); 16];
            for i in 0..16 {
                children[i] = NewPTR::from_usize(node.children[i].as_usize());
            }
            Node {
                children,
                prefix_len: node.prefix_len,
                leaf_mask: node.leaf_mask,
                leaf: NewPTR::from_usize(node.leaf.as_usize()),
                offset: node.offset,
            }
        }).collect();
        Ok(NibbleTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
        })
    }
}

impl<T, PTR: TrieIndex, LEN: TrieIndex> Default for NibbleTrie<T, PTR, LEN> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// Sentinel nib value meaning "positioned at the terminal value of this node."
/// A terminal key is always lexicographically before all of its children —
/// in forward order, terminal is emitted first, then children 0–15.
/// In backward order, children 15→0 are visited first, then terminal.
const TERMINAL_NIB: usize = 16;

pub struct NibbleIter<'a, T, PTR: TrieIndex, LEN: TrieIndex> {
    trie: &'a NibbleTrie<T, PTR, LEN>,
    /// Stack of (arena_index, children_mask, nibble_position) triples.
    ///
    /// - `arena_idx`: index into the arena (which node)
    /// - `mask`: full `children_mask()` of that node, computed once on push.
    /// - `nib`: current position. Values:
    ///   - `usize::MAX`: before-first (initial state, no position)
    ///   - `0..16`: positioned at child slot `nib` (may be leaf or internal)
    ///   - `TERMINAL_NIB (16)`: positioned at the node's terminal value
    stack: Vec<(PTR, u16, usize)>,
}

impl<'a, T, PTR: TrieIndex, LEN: TrieIndex> NibbleIter<'a, T, PTR, LEN> {
    fn new(trie: &'a NibbleTrie<T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].children_mask();
        // If root is terminal, start at the terminal value.
        // Otherwise, start before the first child (usize::MAX).
        let nib = if trie.arena[0].is_terminal() { TERMINAL_NIB } else { usize::MAX };
        NibbleIter { trie, stack: vec![(PTR::zero(), mask, nib)] }
    }

    fn new_last(trie: &'a NibbleTrie<T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut idx: PTR = PTR::zero();
        loop {
            let node = &trie.arena[idx.as_usize()];
            let mask = node.children_mask();
            if mask != 0 {
                // Find the highest set bit (rightmost child)
                let nib = 15 - mask.leading_zeros() as usize;
                stack.push((idx, mask, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    idx = node.children[nib];
                }
            } else if node.is_terminal() {
                // No children, just terminal — position at terminal
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

    /// Descend from internal node `idx` to its rightmost position, pushing
    /// entries onto the stack. For terminal nodes with no children, positions
    /// at the terminal value. Otherwise follows the highest child to the
    /// rightmost leaf.
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
                // Terminal but has children — need to find rightmost child
                // Terminal value comes before children in backward order,
                // so the rightmost position is the rightmost leaf descendant.
            }
            let mask = node.children_mask();
            if mask == 0 {
                if node.is_terminal() {
                    self.stack.push((idx, mask, TERMINAL_NIB));
                }
                return;
            }
            // Find the highest set bit
            let nib = 15 - mask.leading_zeros() as usize;
            self.stack.push((idx, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                idx = node.children[nib];
            }
        }
    }

    /// Scan children of `arena_idx` starting from `start_nib`.
    /// Push the first found child slot onto the stack. If the child
    /// is an internal node, descend to its leftmost position.
    /// Returns `true` if a child was found and pushed.
    #[inline]
    fn push_next_child(&mut self, arena_idx: PTR, mask: u16, start_nib: usize) -> bool {
        // Mask off bits below start_nib, then find the first set bit with TZCNT
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

    /// Pop stack frames and find the next sibling at a higher nibble.
    /// Used when all children at the current level have been exhausted.
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
            // Terminal value — use direct buf offset, no index lookup needed
            let key_len = node.prefix_len.as_usize() / 2;
            let off = node.raw_offset() as usize;
            let key = &self.trie.buf[off..off + key_len];
            let value = &self.trie.values[node.leaf.as_usize() - 1];
            Some((key, value))
        } else if let Some(key_index) = node.leaf_key_index(nib) {
            let key = self.trie.key_slice(key_index);
            let value = &self.trie.values[key_index.as_usize() - 1];
            Some((key, value))
        } else {
            None
        }
    }

    /// Return just the key index at the current cursor position, skipping
    /// the key buffer and value reads. Useful when only the position matters
    /// (e.g., random-access cursor patterns where key/value reads hit scattered
    /// offsets and defeat prefetching).
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

    /// Advance cursor to the next position. Returns `true` if positioned,
    /// `false` if exhausted. Shared navigation for [`next`] and [`next_index`].
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

    /// Advance cursor to the previous position. Returns `true` if positioned,
    /// `false` if exhausted. Shared navigation for [`prev`] and [`prev_index`].
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

            // Scan backward: find highest set bit below nib using LZCNT
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

    /// Advance to the next key in sorted order, returning only its index.
    /// Identical navigation to [`next()`](Self::next) but skips key/value reads.
    #[inline]
    pub fn next_index(&mut self) -> Option<usize> {
        if self.advance_next() { self.current_index() } else { None }
    }

    /// Move to the previous key in sorted order, returning only its index.
    /// Identical navigation to [`prev()`](Self::prev) but skips key/value reads.
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

            // Check if this node is terminal and its key >= seek key
            if node.is_terminal() && node.prefix_len.as_usize() >= max_nib {
                let key_len = node.prefix_len.as_usize() / 2;
                let off = node.raw_offset() as usize;
                let node_key = &self.trie.buf[off..off + key_len];
                if node_key >= key {
                    self.stack.push((node_idx, mask, TERMINAL_NIB));
                    return self.current();
                }
            }

            if node.prefix_len.as_usize() >= max_nib {
                // Key exhausted, terminal key < seek key (or not terminal)
                // Look for a child at or after nibble 0, or backtrack
                if self.push_next_child(node_idx, mask, 0) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let nib = key_nibble_at(key, node.prefix_len.as_usize()) as usize;
            let slot = node.children[nib];
            if slot != PTR::zero() {
                self.stack.push((node_idx, mask, nib));
                if node.is_leaf(nib) {
                    // Check if the leaf key is >= the seek key
                    let leaf_key = self.trie.key_slice(slot);
                    if leaf_key >= key {
                        return self.current();
                    }
                    // Leaf key < seek key: advance past it
                    return self.next();
                } else {
                    node_idx = slot;
                    continue;
                }
            }

            // No exact match — find next higher child, or backtrack
            if self.push_next_child(node_idx, mask, nib + 1) {
                return self.current();
            }
            return self.backtrack_to_next();
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

impl TinyTrieMap for NibbleTrie<usize, u32, u32> {
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
#[path = "tests/nibble_trie.rs"]
mod tests;
