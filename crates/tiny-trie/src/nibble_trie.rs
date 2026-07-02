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
//! # Empty-slot encoding (`OptNz`)
//!
//! Child slots and the `leaf` field use `OptNz<PTR>` — a `#[repr(transparent)]`
//! newtype over `PTR` where the value `0` means "empty" and any nonzero value
//! is a real arena index or key index. `[OptNz<PTR>; 16]` is layout-identical
//! to `[PTR; 16]`, so the SIMD `children_mask` path is reused via a single
//! `repr(transparent)` pointer cast. Real arena child addresses are `>= 1`
//! (the root at arena[0] is never a child target) and real key indices are
//! `>= 1` (index[0] is a dummy entry), so `0` is free as the sentinel.
//!
//! # Key Index Encoding
//!
//! Real keys start at index 1 (index 0 is the dummy entry pointing at buf[0],
//! an unused byte). `values[i]` corresponds to `index[i+1]` (i.e. key index
//! `ki` maps to `values[ki - 1]`).

use crate::ByteKey;
use tiny_trie_trait::TinyTrieMap;
use std::{fmt, marker::PhantomData, simd::{Simd, cmp::SimdPartialEq}};

// ---------------------------------------------------------------------------
// TrieIndex trait
// ---------------------------------------------------------------------------

/// Trait for types used as arena/key indices and prefix lengths in NibbleTrie.
///
/// Implemented for `u8`, `u16`, `u32`, and `u64`. The type parameter `PTR` (pointer
/// type) controls the width of `children`, `leaf`, and arena indices. The type
/// parameter `LEN` (length type) controls the width of `prefix_len` and key
/// lengths in the index.
pub trait TrieIndex: Copy + Clone + Default + PartialEq + Eq + fmt::Debug + 'static {
    /// Convert to `usize` for indexing.
    fn as_usize(self) -> usize;
    /// Maximum representable value (e.g. `u16::MAX` for u16).
    fn max_value() -> usize;
    /// Zero value, used for initial values and as the `OptNz` empty sentinel.
    fn zero() -> Self;
    /// Maximum value used as sentinel for empty slots in `children[]` by the
    /// sibling tries (`fixed_len_nibble_trie`, `nib_trie`). `nibble_trie`
    /// itself uses `0` as its sentinel (see `OptNz`), but keeps this method so
    /// the trait stays shared.
    fn max_value_sentinel() -> Self;
    /// Convert from `usize`. May panic or truncate on overflow in debug builds.
    fn from_usize(n: usize) -> Self;
    /// Compute a 16-bit occupancy mask from a 16-slot children array.
    /// Bit N is set if `children[N]` is not zero.
    fn children_mask(children: &[Self; 16]) -> u16;
}

impl TrieIndex for u8 {
    #[inline] fn as_usize(self) -> usize { self as usize }
    #[inline] fn max_value() -> usize { u8::MAX as usize }
    #[inline] fn zero() -> Self { 0 }
    #[inline] fn max_value_sentinel() -> Self { u8::MAX }
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
    #[inline] fn max_value_sentinel() -> Self { u16::MAX }
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
    #[inline] fn max_value_sentinel() -> Self { u32::MAX }
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
    #[inline] fn max_value_sentinel() -> Self { u64::MAX }
    #[inline] fn from_usize(n: usize) -> Self { n as u64 }
    #[inline] fn children_mask(children: &[Self; 16]) -> u16 {
        crate::simd::children_mask_u64(children)
    }
}

// ---------------------------------------------------------------------------
// OptNz: 0-encoded optional index (no tag byte, layout-identical to PTR)
// ---------------------------------------------------------------------------

/// A nonzero-style optional index: a `#[repr(transparent)]` wrapper over `PTR`
/// where the value `0` denotes "empty" and any nonzero value is a real index.
///
/// `OptNz<PTR>` has the same size and layout as `PTR`, so `[OptNz<PTR>; 16]` is
/// layout-identical to `[PTR; 16]` (used to feed the SIMD `children_mask`). This
/// is the stable, no-`unsafe`-on-access equivalent of `Option<NonZero<PTR>>`.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct OptNz<PTR: TrieIndex>(PTR);

impl<PTR: TrieIndex> OptNz<PTR> {
    /// The empty value (encodes `0`).
    #[inline]
    pub fn empty() -> Self { Self(PTR::zero()) }

    /// Build from a raw `PTR`. Returns `None` if `v` is zero.
    #[inline]
    pub fn new(v: PTR) -> Option<Self> {
        if v == PTR::zero() { None } else { Some(Self(v)) }
    }

    /// Build from a known-nonzero `PTR`. Debug-asserts `v != 0`.
    #[inline]
    pub fn from_index(v: PTR) -> Self {
        debug_assert!(v != PTR::zero(), "OptNz::from_index: zero value");
        Self(v)
    }

    /// The raw underlying `PTR` (zero if empty).
    #[inline]
    pub fn get(self) -> PTR { self.0 }

    /// Whether this slot holds a real index.
    #[inline]
    pub fn is_some(self) -> bool { self.0 != PTR::zero() }

    /// Whether this slot is empty.
    #[inline]
    pub fn is_none(self) -> bool { self.0 == PTR::zero() }
}

impl<PTR: TrieIndex> Default for OptNz<PTR> {
    fn default() -> Self { Self(PTR::zero()) }
}

impl<PTR: TrieIndex> fmt::Debug for OptNz<PTR> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() { write!(f, "-") } else { write!(f, "{:?}", self.0) }
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A single node in the nibble trie arena.
///
/// Generic over `PTR` (pointer/index type for children and arena references)
/// and `LEN` (length type for prefix lengths and key lengths).
///
/// Layout with PTR=u32, LEN=u16: 76 bytes (64 children + 2 prefix_len + 2
/// leaf_mask + 4 leaf + 1 terminal + 3 padding).
/// With PTR=u16, LEN=u16: 40 bytes (32 children + 2 + 2 + 2 + 1 + 1 padding).
#[derive(Copy, Clone)]
pub struct Node<PTR: TrieIndex, LEN: TrieIndex> {
    pub children: [OptNz<PTR>; 16],  // 0 = empty; leaf key index or arena index otherwise
    pub prefix_len: LEN,             // absolute nibble position of the discriminating nibble
    pub leaf_mask: u16,              // bit N set → children[N] is a leaf key index
    pub leaf: OptNz<PTR>,            // key index of a reference/descendant leaf (for retrieval)
    pub terminal: bool,              // true → this node's key ends here (prefix key)
}

impl<PTR: TrieIndex, LEN: TrieIndex> Node<PTR, LEN> {
    pub fn new() -> Self {
        Node {
            children: [OptNz::empty(); 16],
            prefix_len: LEN::zero(),
            leaf_mask: 0,
            leaf: OptNz::empty(),
            terminal: false,
        }
    }

    /// Whether this node is terminal (its own key ends here).
    #[inline]
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Set the terminal flag.
    #[inline]
    fn set_terminal(&mut self, val: bool) {
        self.terminal = val;
    }

    /// Check if nibble slot `nib` is a leaf (key index).
    #[inline]
    pub fn is_leaf(&self, nib: usize) -> bool {
        debug_assert!(nib < 16);
        (self.leaf_mask >> nib) & 1 == 1
    }

    /// Set the leaf flag for nibble slot `nib`.
    #[inline]
    fn set_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 16);
        self.leaf_mask |= 1 << nib;
    }

    /// Clear the leaf flag for nibble slot `nib`.
    #[inline]
    fn clear_leaf(&mut self, nib: usize) {
        debug_assert!(nib < 16);
        self.leaf_mask &= !(1 << nib);
    }

    /// Check if nibble slot `nib` is occupied (holds a child, leaf or internal).
    #[inline]
    pub fn is_occupied(&self, nib: usize) -> bool {
        debug_assert!(nib < 16);
        self.children[nib].is_some()
    }

    /// Store a leaf key index at `nib`. Key index must be nonzero.
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, key_index: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(key_index != PTR::zero(), "zero key index");
        self.set_leaf(nib);
        self.children[nib] = OptNz::from_index(key_index);
    }

    /// Store an arena index at `nib` (internal node reference). Must be nonzero.
    #[inline]
    fn set_internal_child(&mut self, nib: usize, arena_idx: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(arena_idx != PTR::zero(), "zero arena index");
        self.clear_leaf(nib);
        self.children[nib] = OptNz::from_index(arena_idx);
    }

    /// Decode a leaf child at `nib` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize) -> Option<PTR> {
        debug_assert!(nib < 16);
        if self.is_leaf(nib) && self.children[nib].is_some() {
            Some(self.children[nib].get())
        } else {
            None
        }
    }

    /// Compute a 16-bit mask where bit N is set if `children[N]` is occupied.
    /// Reuses the SIMD `children_mask` over the raw `[PTR; 16]` view — sound
    /// because `OptNz<PTR>` is `#[repr(transparent)]` over `PTR`.
    #[inline]
    pub fn children_mask(&self) -> u16 {
        // SAFETY: OptNz<PTR> is #[repr(transparent)] over PTR, so
        // [OptNz<PTR>; 16] has identical layout to [PTR; 16].
        let raw: &[PTR; 16] = unsafe { &*(&self.children as *const [OptNz<PTR>; 16] as *const [PTR; 16]) };
        PTR::children_mask(raw)
    }

    /// Promote this node's PTR type to a wider one.
    /// Child arena indices and leaf key indices are widened via `NewPTR::from_usize`.
    pub fn promote<NewPTR: TrieIndex>(self) -> Node<NewPTR, LEN> {
        let mut children = [OptNz::empty(); 16];
        for i in 0..16 {
            if self.children[i].is_some() {
                children[i] = OptNz::from_index(NewPTR::from_usize(self.children[i].get().as_usize()));
            }
        }
        Node {
            children,
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            leaf: if self.leaf.is_some() {
                OptNz::from_index(NewPTR::from_usize(self.leaf.get().as_usize()))
            } else {
                OptNz::empty()
            },
            terminal: self.terminal,
        }
    }

    /// Demote this node's PTR type to a narrower one.
    /// Returns `Err(self)` if any child index or leaf index doesn't fit
    /// in the narrower type.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<Node<NewPTR, LEN>, Self> {
        for i in 0..16 {
            if self.children[i].is_some() && self.children[i].get().as_usize() > NewPTR::max_value() {
                return Err(self);
            }
        }
        if self.leaf.is_some() && self.leaf.get().as_usize() > NewPTR::max_value() {
            return Err(self);
        }
        let mut children = [OptNz::empty(); 16];
        for i in 0..16 {
            if self.children[i].is_some() {
                children[i] = OptNz::from_index(NewPTR::from_usize(self.children[i].get().as_usize()));
            }
        }
        Ok(Node {
            children,
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            leaf: if self.leaf.is_some() {
                OptNz::from_index(NewPTR::from_usize(self.leaf.get().as_usize()))
            } else {
                OptNz::empty()
            },
            terminal: self.terminal,
        })
    }
}

impl<PTR: TrieIndex, LEN: TrieIndex> fmt::Debug for Node<PTR, LEN> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active: Vec<(usize, &str, PTR)> = (0..16)
            .filter(|&n| self.children[n].is_some())
            .map(|n| {
                let tag = if self.is_leaf(n) { "L" } else { "I" };
                (n, tag, self.children[n].get())
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("{:016b}", self.leaf_mask))
            .field("terminal", &self.terminal)
            .field("leaf", &self.leaf)
            .field("children", &active)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// NibbleTrie
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NibbleTrie<K, T, PTR: TrieIndex = u32, LEN: TrieIndex = u16>
where
    K: ByteKey,
{
    pub arena: Vec<Node<PTR, LEN>>,
    pub buf: Vec<u8>,                // all keys concatenated (no null terminators)
    pub index: Vec<(usize, LEN)>,    // (offset into buf, len) per key — offset is usize, len is compact
    pub values: Vec<T>,              // values[i] ↔ index[i+1]
    _key: PhantomData<K>,
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

/// Outcome of a bounded prefix check: scan nibbles `from..to` and report
/// whether the keys match in that range or diverge at a specific nibble.
/// Unlike `DivergeResult`, this does not scan past `to` and has no
/// `Duplicate` variant — a full match within the bound is `Matches`.
enum PrefixCheck {
    /// The keys match at every nibble position in `from..to`.
    Matches,
    /// The keys diverge at this nibble position (within `from..to`).
    Diverges(usize),
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

fn simd_find_divergence<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize) -> DivergeResult
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

/// Scan nibbles `from..to` of two keys. Returns `Diverges(pos)` if they differ
/// at any nibble in that range, or `Matches` if they agree throughout.
/// An empty range (`from >= to`) is trivially `Matches`.
#[inline]
fn check_prefix(key_a: &[u8], key_b: &[u8], from: usize, to: usize) -> PrefixCheck {
    for nib in from..to {
        if key_nibble_at(key_a, nib) != key_nibble_at(key_b, nib) {
            return PrefixCheck::Diverges(nib);
        }
    }
    PrefixCheck::Matches
}

/// SIMD-accelerated bounded prefix check. Scans nibbles `from..to` and stops
/// at the first divergence within that range. Returns `Matches` if the keys
/// agree throughout, or `Diverges(pos)` at the first differing nibble.
fn simd_check_prefix<const N: usize>(key_a: &[u8], key_b: &[u8], from: usize, to: usize) -> PrefixCheck
{
    if from >= to {
        return PrefixCheck::Matches;
    }

    let from_byte = from / 2;
    let to_byte = (to + 1) / 2; // first byte fully outside the nibble range
    let minlen = key_a.len().min(key_b.len()).min(to_byte);
    let mut i = from_byte;

    while i + N <= minlen {
        let a = Simd::<u8, N>::from_slice(unsafe { key_a.get_unchecked(i..i + N) });
        let b = Simd::<u8, N>::from_slice(unsafe { key_b.get_unchecked(i..i + N) });
        let mask = a.simd_ne(b);
        if mask.any() {
            let diff_byte_idx = i + mask.first_set().unwrap();
            let xor = unsafe { *key_a.get_unchecked(diff_byte_idx) ^ *key_b.get_unchecked(diff_byte_idx) };
            let nib = diverging_nibble(xor, diff_byte_idx);
            if nib < to {
                return PrefixCheck::Diverges(nib);
            }
            // Divergence past the bound — keys match within range
            return PrefixCheck::Matches;
        }
        i += N;
    }

    // Scalar tail
    check_prefix(key_a, key_b, i * 2, to)
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
// NibbleTrie methods
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> NibbleTrie<K, T, PTR, LEN> {
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
            index: vec![(0, LEN::zero())],   // index[0] = dummy entry
            values: Vec::new(),
            _key: PhantomData,
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
        let mut phys_idx: usize = 0;
        let max_nib = key.len() * 2;
        loop {
            let node = &self.arena[phys_idx];
            let prefix_len = node.prefix_len.as_usize();
            // Key nibbles exhausted — check if this node is terminal.
            if prefix_len >= max_nib {
                if node.is_terminal() {
                    let ki = node.leaf.get();
                    let (off, len) = self.index[ki.as_usize()];
                    let key_in_buf = &self.buf[off..off + len.as_usize()];
                    if key.len() == len.as_usize() && simd_eq(&key_in_buf[..key.len()], key) {
                        return Some(ki.as_usize());
                    }
                }
                return None;
            }
            let nib = key_nibble_at(key, prefix_len) as usize;
            if !node.is_occupied(nib) {
                return None;
            }
            if node.is_leaf(nib) {
                let key_index = node.children[nib].get();
                return if simd_eq(self.key_slice(key_index), key) {
                    Some(key_index.as_usize())
                } else {
                    None
                };
            }
            // Internal child — direct arena index
            phys_idx = node.children[nib].get().as_usize();
        }
    }

    /// Unchecked lookup — assumes the key is present in the trie.
    ///
    /// # Safety
    /// The key **must** have been inserted into this trie. All child/leaf indices
    /// encountered during traversal must be valid arena or index entries.
    pub unsafe fn get_unchecked(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut phys_idx: usize = 0;
        let max_nib = key.len() * 2;
        loop {
            let node = unsafe { self.arena.get_unchecked(phys_idx) };
            let prefix_len = node.prefix_len.as_usize();
            if prefix_len >= max_nib {
                debug_assert!(node.is_terminal(), "get_unchecked: key not in set");
                return Some(node.leaf.get().as_usize());
            }
            let nib = unsafe { key_nibble_at_unchecked(key, prefix_len) } as usize;
            let slot = unsafe { node.children.get_unchecked(nib) };
            if slot.is_none() {
                return None;
            }
            if node.is_leaf(nib) {
                return Some(slot.get().as_usize());
            }
            phys_idx = slot.get().as_usize();
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1])
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> NibbleIter<'_, K, T, PTR, LEN> {
        NibbleIter::new(self)
    }

    pub fn iter_last(&self) -> NibbleIter<'_, K, T, PTR, LEN> {
        NibbleIter::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<K>, Vec<T>) {
        let buf = self.buf;
        let keys: Vec<K> = self.index.into_iter().skip(1).map(|(off, len)| {
            K::from_bytes(&buf[off..off + len.as_usize()])
        }).collect();
        (keys, self.values)
    }

    // -----------------------------------------------------------------------
    // Capacity
    // -----------------------------------------------------------------------

    pub fn near_capacity(&self) -> bool {
        // Arena child addresses and key indices are nonzero and must fit in PTR.
        self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value()
    }

    // -----------------------------------------------------------------------
    // Optimize (DFS key-sorted buf rewrite + index/values sort)
    // -----------------------------------------------------------------------

    /// Rewrite `buf` in DFS (key-sorted) order and reorder `index`/`values` to
    /// match, so a forward iteration hits `buf` in ascending memory order.
    ///
    /// The arena topology (child structure) is unchanged — only key indices are
    /// remapped. Idempotent — a second call with no intervening inserts rewrites
    /// the same order.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let mut new_buf = vec![0u8; self.buf.len()];
        let mut cursor: usize = 1; // position 0 is the dummy byte

        // Collect key indices in DFS visitation order for index/values sorting
        let mut dfs_key_order: Vec<PTR> = Vec::new();

        self.walk_optimize(0, &mut new_buf, &mut cursor, &mut dfs_key_order);

        new_buf.truncate(cursor);
        self.buf = new_buf;

        // --- Sort index and values into DFS order ---

        // Build key remap: old key index → new key index (1-based DFS rank).
        // Index 0 is the dummy entry and stays in place.
        let num_keys = dfs_key_order.len();
        let mut key_remap: Vec<usize> = vec![0; self.index.len()];
        key_remap[0] = 0; // dummy stays at 0
        for (new_ki, &old_ki) in dfs_key_order.iter().enumerate() {
            key_remap[old_ki.as_usize()] = new_ki + 1; // 1-based
        }

        // Remap all key index references in the arena (leaf children + leaf pointer).
        for phys in 0..self.arena.len() {
            let mask = self.arena[phys].leaf_mask;
            for nib in 0..16 {
                if (mask >> nib) & 1 == 1 {
                    let old_ki = self.arena[phys].children[nib].get().as_usize();
                    let new_ki = key_remap[old_ki];
                    self.arena[phys].children[nib] = OptNz::from_index(PTR::from_usize(new_ki));
                }
            }
            if self.arena[phys].leaf.is_some() {
                let old_ki = self.arena[phys].leaf.get().as_usize();
                let new_ki = key_remap[old_ki];
                self.arena[phys].leaf = OptNz::from_index(PTR::from_usize(new_ki));
            }
        }

        // Rebuild index in DFS order: new_index[new_ki] = old_index_entry
        let mut new_index: Vec<(usize, LEN)> = vec![(0, LEN::zero()); num_keys + 1];
        new_index[0] = self.index[0]; // keep dummy entry
        for (new_ki, &old_ki) in dfs_key_order.iter().enumerate() {
            new_index[new_ki + 1] = self.index[old_ki.as_usize()];
        }

        // Reorder values to match new key ordering: new_values[new_ki - 1] = old_values[old_ki - 1]
        // Use ptr::read to avoid requiring T: Clone
        let mut new_values = Vec::with_capacity(num_keys);
        unsafe {
            let old_values_ptr = self.values.as_ptr();
            for &old_ki in &dfs_key_order {
                let old_val = std::ptr::read(old_values_ptr.add(old_ki.as_usize() - 1));
                new_values.push(old_val);
            }
        }
        // Old values' elements were moved out via ptr::read, so prevent the old Vec
        // from dropping them. set_len(0) means no destructor calls on elements,
        // then swap so the old (now-empty) Vec gets deallocated normally.
        unsafe { self.values.set_len(0); }
        std::mem::swap(&mut self.values, &mut new_values);
        // new_values now holds the old Vec with len=0 — it will just free its buffer on drop
        self.index = new_index;
    }

    fn walk_optimize(
        &mut self,
        phys_idx: usize,
        new_buf: &mut [u8],
        cursor: &mut usize,
        dfs_key_order: &mut Vec<PTR>,
    ) {
        let node = self.arena[phys_idx]; // copy to avoid borrow conflicts

        // Copy this node's terminal key (if any) first — it sorts before its children.
        if node.is_terminal() {
            let ki = node.leaf.get();
            let (old_off, len) = self.index[ki.as_usize()];
            let start = *cursor;
            new_buf[start..start + len.as_usize()].copy_from_slice(
                &self.buf[old_off..old_off + len.as_usize()]
            );
            self.index[ki.as_usize()].0 = *cursor;
            *cursor += len.as_usize();
            dfs_key_order.push(ki);
        }

        // Visit leaf children in nibble order; recurse into internal children.
        for nib in 0..16 {
            if !node.is_occupied(nib) {
                continue;
            }
            if node.is_leaf(nib) {
                let ki = node.children[nib].get();
                let (old_off, len) = self.index[ki.as_usize()];
                let start = *cursor;
                new_buf[start..start + len.as_usize()].copy_from_slice(
                    &self.buf[old_off..old_off + len.as_usize()]
                );
                self.index[ki.as_usize()].0 = *cursor;
                *cursor += len.as_usize();
                dfs_key_order.push(ki);
            } else {
                self.walk_optimize(node.children[nib].get().as_usize(), new_buf, cursor, dfs_key_order);
            }
        }
    }
}

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> Default for NibbleTrie<K, T, PTR, LEN> {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> NibbleTrie<K, T, PTR, LEN> {
    pub fn insert(&mut self, key: K, value: T) -> Result<usize, ()> {
        let key_bytes = key.as_bytes();
        // Overflow checks: arena/key indices must fit in PTR (nonzero, so < max).
        if self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value() {
            return Err(());
        }
        if key_bytes.len() * 2 > LEN::max_value() {
            return Err(());
        }

        let new_index = PTR::from_usize(self.index.len());
        let key_len = LEN::from_usize(key_bytes.len());
        let offset = self.buf.len() as usize;
        self.buf.extend_from_slice(key_bytes);
        self.index.push((offset, key_len));
        self.values.push(value);

        let max_nib = key_bytes.len() * 2;

        if self.arena.is_empty() {
            return Ok(self.insert_into_empty_trie(key_bytes, new_index, max_nib));
        }

        let mut phys_idx: usize = 0;
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[phys_idx];
            let ki = node.leaf.get();
            let (off, ref_len) = self.index[ki.as_usize()];
            let ref_key = &self.buf[off..off + ref_len.as_usize()];
            let prefix_len = node.prefix_len.as_usize();

            match simd_check_prefix::<8>(key_bytes, ref_key, confirmed, prefix_len) {
                PrefixCheck::Diverges(diverge) => {
                    return Ok(self.split_node_before_prefix(
                        phys_idx, diverge, new_index, key_bytes, max_nib,
                    ));
                }
                PrefixCheck::Matches => {
                    if max_nib == prefix_len {
                        if key_bytes.len() == ref_key.len() {
                            self.rollback_last_insert();
                            return Err(());
                        }
                        self.arena[phys_idx].set_terminal(true);
                        self.arena[phys_idx].leaf = OptNz::from_index(new_index);
                        return Ok(new_index.as_usize());
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(key_bytes, prefix_len) as usize;
                    if !node.is_occupied(nib) {
                        // Empty slot — new key diverges here
                        self.arena[phys_idx].set_leaf_child(nib, new_index);
                        return Ok(new_index.as_usize());
                    }

                    if node.is_leaf(nib) {
                        let slot = node.children[nib].get();
                        return self.split_leaf_child(
                            nib, phys_idx, slot, new_index, key_bytes, max_nib, confirmed,
                        );
                    }

                    phys_idx = node.children[nib].get().as_usize();
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
            let mut root = Node::new();
            root.set_terminal(true);
            root.leaf = OptNz::from_index(new_index);
            self.arena.push(root);
            return new_index.as_usize();
        }
        let first_nib = key_nibble_at(key, 0) as usize;
        let mut root = Node::new();
        root.set_leaf_child(first_nib, new_index);
        root.leaf = OptNz::from_index(new_index);
        self.arena.push(root);
        new_index.as_usize()
    }

    #[inline]
    fn split_node_before_prefix(
        &mut self,
        phys_idx: usize,
        diverge: usize,
        new_index: PTR,
        key: &[u8],
        max_nib: usize,
    ) -> usize {
        let node = &self.arena[phys_idx];
        let ki = node.leaf.get();
        let (off, ref_len) = self.index[ki.as_usize()];
        let ref_key = &self.buf[off..off + ref_len.as_usize()];

        let new_nib = key_nibble_at(key, diverge) as usize;
        let ref_nib = key_nibble_at(ref_key, diverge) as usize;

        let mut new_parent = Node::new();
        new_parent.prefix_len = LEN::from_usize(diverge);

        if diverge >= max_nib {
            new_parent.set_terminal(true);
            new_parent.leaf = OptNz::from_index(new_index);
        } else {
            new_parent.set_leaf_child(new_nib, new_index);
            new_parent.leaf = OptNz::from_index(new_index);
        }

        let old_node = std::mem::replace(&mut self.arena[phys_idx], new_parent);
        let old_addr = PTR::from_usize(self.arena.len()); // new node index (>= 1)
        self.arena.push(old_node);

        self.arena[phys_idx].set_internal_child(ref_nib, old_addr);

        new_index.as_usize()
    }

    #[inline]
    fn split_leaf_child(
        &mut self,
        nib: usize,
        phys_idx: usize,
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
                let mut split_node = Node::new();
                split_node.prefix_len = LEN::from_usize(d);

                if d >= max_nib {
                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                    split_node.set_terminal(true);
                    split_node.leaf = OptNz::from_index(new_index);
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                } else if d >= existing_key.len() * 2 {
                    let new_nib = key_nibble_at(key, d) as usize;
                    split_node.set_terminal(true);
                    split_node.leaf = OptNz::from_index(existing_key_index);
                    split_node.set_leaf_child(new_nib, new_index);
                } else {
                    let new_nib = key_nibble_at(key, d) as usize;
                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                    debug_assert_ne!(new_nib, exist_nib);
                    split_node.set_leaf_child(new_nib, new_index);
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                    split_node.leaf = OptNz::from_index(existing_key_index);
                }

                let split_addr = PTR::from_usize(self.arena.len());
                self.arena.push(split_node);
                self.arena[phys_idx].set_internal_child(nib, split_addr);

                Ok(new_index.as_usize())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PTR width conversions (promote/demote)
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> NibbleTrie<K, T, PTR, LEN> {
    /// Promote the arena index type to a wider PTR.
    /// All child indices and leaf key indices are widened via `NewPTR::from_usize`.
    pub fn promote<NewPTR: TrieIndex>(self) -> NibbleTrie<K, T, NewPTR, LEN> {
        let arena = self.arena.into_iter().map(|node| node.promote()).collect();
        NibbleTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
            _key: PhantomData,
        }
    }

    /// Demote the arena index type to a narrower PTR.
    /// Returns `Err(self)` if any index doesn't fit in the narrower type.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<NibbleTrie<K, T, NewPTR, LEN>, Self> {
        if self.arena.len() > NewPTR::max_value() || self.index.len() > NewPTR::max_value() {
            return Err(self);
        }
        let arena = self.arena.into_iter().map(|node| {
            node.demote().expect("demote capacity check should have caught this")
        }).collect();
        Ok(NibbleTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
            _key: PhantomData,
        })
    }
}


// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// Sentinel nib value meaning "positioned at the terminal value of this node."
const TERMINAL_NIB: usize = 16;

pub struct NibbleIter<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> {
    trie: &'a NibbleTrie<K, T, PTR, LEN>,
    /// Stack of (arena_index, occupancy_mask, nibble_position) tuples.
    stack: Vec<(PTR, u16, usize)>,
}

impl<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> NibbleIter<'a, K, T, PTR, LEN> {
    fn new(trie: &'a NibbleTrie<K, T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].children_mask();
        let nib = if trie.arena[0].is_terminal() { TERMINAL_NIB } else { usize::MAX };
        NibbleIter { trie, stack: vec![(PTR::zero(), mask, nib)] }
    }

    fn new_last(trie: &'a NibbleTrie<K, T, PTR, LEN>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut phys_idx: usize = 0;
        loop {
            let node = &trie.arena[phys_idx];
            let mask = node.children_mask();
            if mask != 0 {
                let nib = 15 - mask.leading_zeros() as usize;
                stack.push((PTR::from_usize(phys_idx), mask, nib));
                if node.is_leaf(nib) {
                    break;
                } else {
                    phys_idx = node.children[nib].get().as_usize();
                }
            } else if node.is_terminal() {
                stack.push((PTR::from_usize(phys_idx), mask, TERMINAL_NIB));
                break;
            } else {
                break;
            }
        }
        NibbleIter { trie, stack }
    }

    fn descend_first(&mut self, mut phys_idx: usize) {
        loop {
            let node = &self.trie.arena[phys_idx];
            if node.is_terminal() {
                let mask = node.children_mask();
                self.stack.push((PTR::from_usize(phys_idx), mask, TERMINAL_NIB));
                return;
            }
            let mask = node.children_mask();
            debug_assert!(mask != 0, "descend_first: non-terminal node with no children");
            let nib = mask.trailing_zeros() as usize;
            self.stack.push((PTR::from_usize(phys_idx), mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                phys_idx = node.children[nib].get().as_usize();
            }
        }
    }

    fn descend_last(&mut self, mut phys_idx: usize) {
        loop {
            let node = &self.trie.arena[phys_idx];
            if node.is_terminal() {
                let mask = node.children_mask();
                if mask == 0 {
                    self.stack.push((PTR::from_usize(phys_idx), mask, TERMINAL_NIB));
                    return;
                }
            }
            let mask = node.children_mask();
            if mask == 0 {
                if node.is_terminal() {
                    self.stack.push((PTR::from_usize(phys_idx), mask, TERMINAL_NIB));
                }
                return;
            }
            let nib = 15 - mask.leading_zeros() as usize;
            self.stack.push((PTR::from_usize(phys_idx), mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                phys_idx = node.children[nib].get().as_usize();
            }
        }
    }

    #[inline]
    fn push_next_child(&mut self, encoded: PTR, mask: u16, start_nib: usize) -> bool {
        let shifted = if start_nib >= 16 { 0u16 } else { mask >> start_nib };
        if shifted == 0 {
            return false;
        }
        let nib = start_nib + shifted.trailing_zeros() as usize;
        debug_assert!(nib < 16);
        debug_assert!(mask & (1 << nib) != 0);
        let phys_idx = encoded.as_usize();
        self.stack.push((encoded, mask, nib));
        if !self.trie.arena[phys_idx].is_leaf(nib) {
            let addr = self.trie.arena[phys_idx].children[nib].get().as_usize();
            self.descend_first(addr);
        }
        true
    }

    #[inline]
    fn backtrack_to_next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (parent_encoded, parent_mask, child_nib) = self.stack.pop()?;
            if self.push_next_child(parent_encoded, parent_mask, child_nib + 1) {
                return self.current();
            }
        }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let (encoded, _, nib) = self.stack.last()?;
        if *nib == usize::MAX {
            return None;
        }
        let phys_idx = encoded.as_usize();
        let node = &self.trie.arena[phys_idx];
        if *nib == TERMINAL_NIB {
            let ki = node.leaf.get();
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
        let (encoded, _, _) = self.stack.last()?;
        let phys_idx = encoded.as_usize();
        let node = &self.trie.arena[phys_idx];
        if nib == TERMINAL_NIB {
            Some(node.leaf.get().as_usize())
        } else {
            node.leaf_key_index(nib).map(|ki| ki.as_usize())
        }
    }

    #[inline]
    fn advance_next(&mut self) -> bool {
        loop {
            let (encoded, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                if self.push_next_child(encoded, mask, 0) {
                    return true;
                }
                continue;
            }

            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };
            if self.push_next_child(encoded, mask, search_start) {
                return true;
            }
        }
    }

    #[inline]
    fn advance_prev(&mut self) -> bool {
        loop {
            let (encoded, mask, nib) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                continue;
            }

            if nib == 0 || nib == usize::MAX {
                let phys_idx = encoded.as_usize();
                if self.trie.arena[phys_idx].is_terminal() {
                    self.stack.push((encoded, mask, TERMINAL_NIB));
                    return true;
                }
                continue;
            }

            let mask_below = mask & ((1 << nib) - 1);
            if mask_below != 0 {
                let prev_nib = 15 - mask_below.leading_zeros() as usize;
                let phys_idx = encoded.as_usize();
                self.stack.push((encoded, mask, prev_nib));
                if !self.trie.arena[phys_idx].is_leaf(prev_nib) {
                    let addr = self.trie.arena[phys_idx].children[prev_nib].get().as_usize();
                    self.descend_last(addr);
                }
                return true;
            }

            let phys_idx = encoded.as_usize();
            if self.trie.arena[phys_idx].is_terminal() {
                self.stack.push((encoded, mask, TERMINAL_NIB));
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
        let mut phys_idx: usize = 0;
        let max_nib = key.len() * 2;

        loop {
            let node = &self.trie.arena[phys_idx];
            let mask = node.children_mask();

            if node.is_terminal() && node.prefix_len.as_usize() >= max_nib {
                let ki = node.leaf.get();
                let (off, len) = self.trie.index[ki.as_usize()];
                let node_key = &self.trie.buf[off..off + len.as_usize()];
                if node_key >= key {
                    self.stack.push((PTR::from_usize(phys_idx), mask, TERMINAL_NIB));
                    return self.current();
                }
            }

            if node.prefix_len.as_usize() >= max_nib {
                if self.push_next_child(PTR::from_usize(phys_idx), mask, 0) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let nib = key_nibble_at(key, node.prefix_len.as_usize()) as usize;
            if !node.is_occupied(nib) {
                // No child at this nibble — find next higher child, or backtrack
                if self.push_next_child(PTR::from_usize(phys_idx), mask, nib + 1) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            self.stack.push((PTR::from_usize(phys_idx), mask, nib));
            if node.is_leaf(nib) {
                let leaf_key = self.trie.key_slice(node.children[nib].get());
                if leaf_key >= key {
                    return self.current();
                }
                // Leaf key < seek key: advance past it
                return self.next();
            } else {
                phys_idx = node.children[nib].get().as_usize();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TinyTrieMap implementations
// ---------------------------------------------------------------------------

impl TinyTrieMap for NibbleTrie<Vec<u8>, usize> {
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

impl TinyTrieMap for NibbleTrie<Vec<u8>, usize, u32, u32> {
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