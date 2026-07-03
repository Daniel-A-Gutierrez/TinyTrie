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
use std::{fmt, marker::PhantomData, num::NonZero, simd::{Simd, cmp::SimdPartialEq}};

/// One slot of the sparse `index`: the buf offset (>= 1; buf[0] is the dummy byte),
/// the key length, and the value inline. `None` slots are gaps.
pub type Slot<LEN, T> = (NonZero<usize>, LEN, T);

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
    pub index: Vec<Option<Slot<LEN, T>>>, // sparse: position == key index; None = gap; [0] = dummy
    pub n_keys: usize,               // live key count (replaces index.len()-1)
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
        let (off, len, _) = self.index[key_index.as_usize()].as_ref().unwrap();
        &self.buf[off.get()..off.get() + len.as_usize()]
    }

    pub fn new() -> Self {
        NibbleTrie {
            arena: Vec::new(),
            buf: vec![0],           // buf[0] = dummy (unused byte)
            index: vec![None],      // index[0] = dummy gap
            n_keys: 0,
            _key: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.n_keys
    }

    pub fn is_empty(&self) -> bool {
        self.n_keys == 0
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
                    let (off, len, _) = self.index[ki.as_usize()].as_ref().unwrap();
                    let off = off.get();
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
        self.get(key).map(|idx| &self.index[idx].as_ref().unwrap().2)
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    /// An internal tree-walking cursor, used to position the public `Cursor`
    /// (via `seek`) and by `bump_walk` (via `seek` + `stack`).
    pub(crate) fn walk_iter(&self) -> NibbleIter<'_, K, T, PTR, LEN> {
        NibbleIter::new(self)
    }

    /// Public forward cursor: parked *before* the first key (so `current()` is
    /// `None` and `next()` yields the first key). A linear scan over the sparse
    /// `index`, skipping `None` gaps.
    pub fn iter(&self) -> Cursor<'_, K, T, PTR, LEN> {
        Cursor::new(self)
    }

    /// Public reverse cursor: parked *on* the last key (`current()` returns it,
    /// `prev()` walks backward). Linear scan over `index`.
    pub fn iter_last(&self) -> Cursor<'_, K, T, PTR, LEN> {
        Cursor::new_last(self)
    }

    pub fn into_keys_values(self) -> (Vec<K>, Vec<T>) {
        let buf = self.buf;
        let mut keys: Vec<K> = Vec::with_capacity(self.n_keys);
        let mut values: Vec<T> = Vec::with_capacity(self.n_keys);
        for (i, slot) in self.index.into_iter().enumerate() {
            if i == 0 { continue; } // dummy
            if let Some((off, len, val)) = slot {
                keys.push(K::from_bytes(&buf[off.get()..off.get() + len.as_usize()]));
                values.push(val);
            }
        }
        (keys, values)
    }

    // -----------------------------------------------------------------------
    // Capacity
    // -----------------------------------------------------------------------

    pub fn near_capacity(&self) -> bool {
        // Arena child addresses and key indices are nonzero and must fit in PTR.
        self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value()
    }

    // -----------------------------------------------------------------------
    // Optimize (DFS key-sorted buf rewrite + sparse 2*i+1 index re-spread)
    // -----------------------------------------------------------------------

    /// Rewrite `buf` in DFS (key-sorted) order and re-spread `index` into a
    /// sparse layout: a fresh vec of capacity `2*n+1` with each key placed at
    /// slot `2*i+1` (DFS rank `i`), leaving even slots as `None` gaps. Forward
    /// iteration then hits `buf` in ascending memory order, and the gaps give
    /// future inserts room to shift into without re-sorting.
    ///
    /// Also (re)establishes the leftmost-`leaf` invariant: every node's `leaf`
    /// is set to the key index of the leftmost key in its subtree. The arena
    /// topology (child structure) is unchanged — only key indices are remapped.
    /// Idempotent.
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let n = self.n_keys;
        let cap = 2 * n + 1;
        // Build a gap-filled vec without requiring T: Clone (vec![None; cap] would).
        let mut new_index: Vec<Option<Slot<LEN, T>>> = (0..cap).map(|_| None).collect();
        let mut new_buf: Vec<u8> = Vec::with_capacity(self.buf.len());
        new_buf.push(0); // dummy byte at position 0
        let mut cursor: usize = 1;
        let mut i: usize = 0; // global DFS rank

        self.walk_optimize(0, &mut new_index, &mut new_buf, &mut cursor, &mut i);

        new_buf.truncate(cursor);
        self.buf = new_buf;
        self.index = new_index;
    }

    /// DFS walk that places each key at `2*i+1` in `new_index`, copies its bytes
    /// contiguously into `new_buf`, rewrites the arena's key-index references
    /// (`children[nib]` for leaf children, `leaf` for the node's leftmost), and
    /// returns the slot of the leftmost key placed in this subtree.
    fn walk_optimize(
        &mut self,
        phys_idx: usize,
        new_index: &mut Vec<Option<Slot<LEN, T>>>,
        new_buf: &mut Vec<u8>,
        cursor: &mut usize,
        i: &mut usize,
    ) -> usize {
        let node = self.arena[phys_idx]; // copy to avoid borrow conflicts
        let mut first: Option<usize> = None;

        // This node's own terminal key sorts before all its descendants.
        if node.is_terminal() {
            let slot = 2 * *i + 1;
            *i += 1;
            let old_ki = node.leaf.get().as_usize();
            let (off, len, val) = self.index[old_ki].take().unwrap();
            let old_off = off.get();
            let start = *cursor;
            new_buf.resize(start + len.as_usize(), 0);
            new_buf[start..start + len.as_usize()]
                .copy_from_slice(&self.buf[old_off..old_off + len.as_usize()]);
            *cursor = start + len.as_usize();
            new_index[slot] = Some((NonZero::new(start).unwrap(), len, val));
            first = Some(slot);
        }

        // Visit children in nibble order (== sorted order); leaf children become
        // keys, internal children are recursed into.
        for nib in 0..16 {
            if !node.is_occupied(nib) {
                continue;
            }
            if node.is_leaf(nib) {
                let slot = 2 * *i + 1;
                *i += 1;
                let old_ki = node.children[nib].get().as_usize();
                let (off, len, val) = self.index[old_ki].take().unwrap();
                let old_off = off.get();
                let start = *cursor;
                new_buf.resize(start + len.as_usize(), 0);
                new_buf[start..start + len.as_usize()]
                    .copy_from_slice(&self.buf[old_off..old_off + len.as_usize()]);
                *cursor = start + len.as_usize();
                new_index[slot] = Some((NonZero::new(start).unwrap(), len, val));
                self.arena[phys_idx].children[nib] = OptNz::from_index(PTR::from_usize(slot));
                if first.is_none() {
                    first = Some(slot);
                }
            } else {
                let child_first =
                    self.walk_optimize(node.children[nib].get().as_usize(), new_index, new_buf, cursor, i);
                if first.is_none() {
                    first = Some(child_first);
                }
            }
        }

        let leftmost = first.expect("walk_optimize: node must have at least one key in subtree");
        self.arena[phys_idx].leaf = OptNz::from_index(PTR::from_usize(leftmost));
        leftmost
    }
}

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> Default for NibbleTrie<K, T, PTR, LEN> {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Insertion (Stage B: shift-based slot allocation + bump walk)
// ---------------------------------------------------------------------------

/// The resolved insertion case, produced by a non-mutating descent
/// (`find_insert_case`) BEFORE any arena/index mutation. All sub-cases and
/// nibble values are read from the pre-mutation tree, so the case stays valid
/// across the slot shift + bump (which only remap key indices, never arena
/// topology or nibble positions). The one exception — `SplitLeaf`'s existing
/// key index — is re-read from the arena in `execute_case` (post-bump) rather
/// than captured here, because that leaf slot may be the successor `p` and get
/// bumped from `p` to `p+1`.
enum Case {
    /// New key is a prefix of the node's reference key → `phys` becomes terminal.
    Terminal { phys: usize },
    /// New key diverges at `phys.prefix_len` into an empty nibble slot → leaf child.
    NewLeafChild { phys: usize, nib: usize },
    /// New key diverges from the node's reference key mid-prefix → split `phys`
    /// into a new parent (at `diverge`) holding the new key and the old subtree.
    SplitNode {
        phys: usize,
        diverge: usize,
        new_is_terminal: bool,
        new_nib: usize,
        ref_nib: usize,
        new_is_leftmost: bool,
    },
    /// New key diverges from an existing leaf child of `phys` at nibble `nib`
    /// → replace that leaf with a new split node holding both keys.
    SplitLeaf {
        phys: usize,
        nib: usize,
        d: usize,
        new_is_terminal: bool,
        existing_is_terminal: bool,
        new_nib: usize,
        exist_nib: usize,
        new_is_leftmost: bool,
    },
}

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

        // 90% capacity trigger: when the sparse index or buf is nearly full,
        // re-spread into a fresh 2n+1 layout so future shifts have gaps to land in.
        // Skip the re-spread if `2n+1` would overflow PTR (`2n < max` ⟺ `2n+1 <= max`,
        // then the trie is simply near its index capacity; the overflow checks below
        // return Err instead).
        if self.needs_optimize() && 2 * self.n_keys < PTR::max_value() {
            self.optimize();
        }
        // Overflow checks: arena/key indices must fit in PTR (nonzero, so < max).
        if self.arena.len() >= PTR::max_value() || self.index.len() >= PTR::max_value() {
            return Err(());
        }

        let key_len = LEN::from_usize(key_bytes.len());
        let off = self.buf.len();
        self.buf.extend_from_slice(key_bytes);
        // buf[0] is the dummy byte, so every real key offset is >= 1 → NonZero.
        self.n_keys += 1;
        let max_nib = key_bytes.len() * 2;

        if self.arena.is_empty() {
            return Ok(self.insert_into_empty_trie(off, key_len, value, key_bytes, max_nib));
        }

        // 1. Detect: non-mutating descent resolves the case + the descent path.
        let (case, path) = match self.find_insert_case(key_bytes, max_nib) {
            Ok(c) => c,
            Err(()) => {
                // Duplicate — no slot was pushed yet (slot alloc happens below),
                // so rollback just drops the buf extend and the key count.
                self.buf.truncate(off);
                self.n_keys -= 1;
                return Err(());
            }
        };

        // 2. Compute p: the slot the successor key currently occupies (the new
        //    key sorts into position p, shifting [p, p+n-1] right). None = END
        //    (new key is the largest → append, no shift, no bump).
        let p_opt = self.compute_p(&case, &path);
        let (p, n) = match p_opt {
            None => {
                let p = self.index.len();
                self.index
                    .push(Some((NonZero::new(off).unwrap(), key_len, value)));
                self.execute_case(case, p, &path);
                return Ok(p);
            }
            Some(p) => {
                // Scan forward from p, counting occupied slots until the first gap.
                // (All keys from p onward are contiguous until a None gap — the
                // successor and its trailing run that must shift right by one.)
                let mut n = 0;
                while p + n < self.index.len() && self.index[p + n].is_some() {
                    n += 1;
                }
                (p, n)
            }
        };

        // Ensure room for the shift: the trailing gap may lie past `index.len()`.
        if p + n >= self.index.len() {
            self.index.push(None);
        }

        if n > 0 {
            // 3. Position a forward walk at the successor key (slot p) by seeking.
            //    The seek borrows self immutably; copy out the (all-Copy) stack
            //    and drop the borrow before mutating.
            let succ_bytes = {
                let (soff, slen, _) = self.index[p].as_ref().unwrap();
                self.buf[soff.get()..soff.get() + slen.as_usize()].to_vec()
            };
            let stack: Vec<(usize, u16, usize)> = {
                let mut it = self.walk_iter();
                it.seek(&succ_bytes);
                debug_assert_eq!(
                    it.current_index(),
                    Some(p),
                    "seek must land on the successor slot"
                );
                it.stack
                    .iter()
                    .map(|&(a, b, c)| (a.as_usize(), b, c))
                    .collect()
            };

            // 4. Bump arena refs whose key index ∈ [p, p+n-1] (every shifted key's
            //    structural ptr + every node whose leftmost is a shifted key).
            self.bump_walk(stack, p, n);

            // 5. Shift the slots right by one. A `take()` walk from the right end
            //    (not `copy_within`, which needs `T: Copy`) — a true element-wise
            //    move that leaves `None` at `p` for the new slot.
            for i in (0..n).rev() {
                self.index[p + i + 1] = self.index[p + i].take();
            }
        }

        // 6. Place the new key's slot at p.
        self.index[p] = Some((NonZero::new(off).unwrap(), key_len, value));

        // 7. Wire the new key into the arena at slot p (re-reading any
        //    bump-sensitive leaf index from the arena post-bump), then propagate
        //    the leftmost-`leaf` invariant up the spine.
        self.execute_case(case, p, &path);
        Ok(p)
    }

    /// 90% capacity trigger. Measures fill as `n_keys / index.capacity()` (NOT
    /// `len / capacity`): after `optimize`, `len == capacity` because the gaps
    /// are real `None` slots, so `len` would always read as 100% full.
    #[inline]
    fn needs_optimize(&self) -> bool {
        let idx_cap = self.index.capacity();
        let buf_cap = self.buf.capacity();
        (idx_cap > 0 && 10 * self.n_keys > 9 * idx_cap)
            || (buf_cap > 0 && 10 * self.buf.len() > 9 * buf_cap)
    }

    /// Non-mutating descent mirroring the lookup walk, but it RECORDS the
    /// resolved `Case` and descent `path` instead of mutating. Reads the
    /// reference/existing keys here (before any shift moves their slots).
    /// Returns `Err(())` for duplicates.
    fn find_insert_case(
        &self,
        key: &[u8],
        max_nib: usize,
    ) -> Result<(Case, Vec<(usize, usize)>), ()> {
        let mut phys_idx: usize = 0;
        let mut confirmed: usize = 0;
        // Path of (ancestor_phys, nib_used_to_descend) from root to the current
        // node, used to propagate the leftmost-`leaf` invariant up the spine.
        let mut path: Vec<(usize, usize)> = Vec::new();

        loop {
            let node = &self.arena[phys_idx];
            let ki = node.leaf.get();
            let (off, ref_len, _) = self.index[ki.as_usize()].as_ref().unwrap();
            let off = off.get();
            let ref_key = &self.buf[off..off + ref_len.as_usize()];
            let prefix_len = node.prefix_len.as_usize();

            match simd_check_prefix::<8>(key, ref_key, confirmed, prefix_len) {
                PrefixCheck::Diverges(diverge) => {
                    let new_nib = key_nibble_at(key, diverge) as usize;
                    let ref_nib = key_nibble_at(ref_key, diverge) as usize;
                    let new_is_terminal = diverge >= max_nib;
                    let new_is_leftmost = new_is_terminal || new_nib < ref_nib;
                    return Ok((
                        Case::SplitNode {
                            phys: phys_idx,
                            diverge,
                            new_is_terminal,
                            new_nib,
                            ref_nib,
                            new_is_leftmost,
                        },
                        path,
                    ));
                }
                PrefixCheck::Matches => {
                    if max_nib == prefix_len {
                        if key.len() == ref_key.len() {
                            return Err(()); // exact duplicate
                        }
                        // New key is a prefix of the ref key → node becomes terminal.
                        return Ok((Case::Terminal { phys: phys_idx }, path));
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(key, prefix_len) as usize;
                    if !node.is_occupied(nib) {
                        // Empty slot — new key diverges here as a leaf child.
                        return Ok((Case::NewLeafChild { phys: phys_idx, nib }, path));
                    }

                    if node.is_leaf(nib) {
                        // Split the existing leaf child: resolve divergence here.
                        path.push((phys_idx, nib));
                        let existing_key_index = node.children[nib].get();
                        let (eo, elen, _) =
                            self.index[existing_key_index.as_usize()].as_ref().unwrap();
                        let existing_key = &self.buf[eo.get()..eo.get() + elen.as_usize()];
                        match simd_find_divergence::<8>(key, existing_key, confirmed) {
                            DivergeResult::Duplicate => return Err(()),
                            DivergeResult::At(d) => {
                                let new_is_terminal = d >= max_nib;
                                let existing_is_terminal = d >= existing_key.len() * 2;
                                let new_nib = key_nibble_at(key, d) as usize;
                                let exist_nib = key_nibble_at(existing_key, d) as usize;
                                let new_is_leftmost = if new_is_terminal {
                                    true
                                } else if existing_is_terminal {
                                    false
                                } else {
                                    new_nib < exist_nib
                                };
                                return Ok((
                                    Case::SplitLeaf {
                                        phys: phys_idx,
                                        nib,
                                        d,
                                        new_is_terminal,
                                        existing_is_terminal,
                                        new_nib,
                                        exist_nib,
                                        new_is_leftmost,
                                    },
                                    path,
                                ));
                            }
                        }
                    }

                    path.push((phys_idx, nib));
                    phys_idx = node.children[nib].get().as_usize();
                }
            }
        }
    }

    /// Compute `p`: the key index of the successor (the leftmost key that sorts
    /// STRICTLY AFTER the new key). The new key takes slot `p`, shifting the
    /// successor and its trailing run right. `None` means the new key is the
    /// largest (END — append, no shift). Reads only pre-mutation state.
    fn compute_p(&self, case: &Case, path: &[(usize, usize)]) -> Option<usize> {
        match case {
            Case::Terminal { phys } => Some(self.arena[*phys].leaf.get().as_usize()),
            Case::NewLeafChild { phys, nib } => self.right_anchor(*phys, *nib, path),
            Case::SplitNode {
                phys,
                new_is_terminal,
                new_nib,
                ref_nib,
                ..
            } => {
                if *new_is_terminal || *new_nib < *ref_nib {
                    // New key is the new leftmost of `phys`'s subtree → successor
                    // is the old leftmost (the ref key), read before mutation.
                    Some(self.arena[*phys].leaf.get().as_usize())
                } else {
                    self.subtree_successor(path)
                }
            }
            Case::SplitLeaf {
                phys,
                nib,
                new_is_terminal,
                existing_is_terminal,
                new_nib,
                exist_nib,
                ..
            } => {
                let existing_key_index = self.arena[*phys].children[*nib].get().as_usize();
                if *new_is_terminal {
                    Some(existing_key_index)
                } else if *existing_is_terminal {
                    self.right_anchor(*phys, *nib, path)
                } else if *new_nib < *exist_nib {
                    Some(existing_key_index)
                } else {
                    self.right_anchor(*phys, *nib, path)
                }
            }
        }
    }

    /// The leftmost key index of the next-higher subtree at `phys` (relative to
    /// nib `nib`), i.e. the successor of a key ending at `phys` via `nib`. Falls
    /// back to `subtree_successor` if `phys` has no higher occupied nibble.
    /// Uses the leftmost-`leaf` invariant: an internal child's `leaf` is its
    /// subtree's leftmost key index.
    fn right_anchor(&self, phys: usize, nib: usize, path: &[(usize, usize)]) -> Option<usize> {
        let mask = self.arena[phys].children_mask();
        let higher = if nib >= 15 { 0u16 } else { mask & !((1u16 << (nib + 1)) - 1) };
        if higher != 0 {
            let next_nib = higher.trailing_zeros() as usize;
            let r = self.arena[phys].children[next_nib].get();
            Some(if self.arena[phys].is_leaf(next_nib) {
                r.as_usize()
            } else {
                self.arena[r.as_usize()].leaf.get().as_usize()
            })
        } else {
            self.subtree_successor(path)
        }
    }

    /// Walk up `path` (deepest first); at each `(parent, nib)` find a higher
    /// occupied nibble than the one descended through. The leftmost of that
    /// higher subtree is the successor. `None` = no higher ancestor nibble =
    /// the new key is the largest (END).
    fn subtree_successor(&self, path: &[(usize, usize)]) -> Option<usize> {
        for &(parent, nib) in path.iter().rev() {
            let mask = self.arena[parent].children_mask();
            let higher = if nib >= 15 { 0u16 } else { mask & !((1u16 << (nib + 1)) - 1) };
            if higher != 0 {
                let next_nib = higher.trailing_zeros() as usize;
                let r = self.arena[parent].children[next_nib].get();
                return Some(if self.arena[parent].is_leaf(next_nib) {
                    r.as_usize()
                } else {
                    self.arena[r.as_usize()].leaf.get().as_usize()
                });
            }
        }
        None
    }

    /// Bump every arena ref whose key index ∈ [lo, lo+n-1]: each shifted key's
    /// structural ptr (terminal → `node.leaf`, leaf child → `node.children[nib]`)
    /// and every node whose `leaf` (leftmost) is a shifted key.
    ///
    /// Done as a forward DFS walk from slot `lo` for exactly `n` keys, mirroring
    /// `NibbleIter`'s advance/push_next_child/descend_first but with direct
    /// `&mut self` arena mutation. Navigation stays safe mid-walk: internal
    /// `children[nib]` are arena indices (unchanged by a key-index shift); leaf
    /// `children[nib]` are terminal for navigation; `leaf` is never traversed.
    ///
    /// Bumping rule (unified, avoids double-bumping terminal nodes whose
    /// `leaf` IS their structural ptr): bump `leaf` of EVERY touched node whose
    /// `leaf ∈ [lo,hi]` (seek-path ancestors + nodes entered via descend_first),
    /// and bump `children[nib]` for each visited leaf-child key. Terminal keys'
    /// structural ptr is their node's `leaf`, bumped once by the first rule.
    fn bump_walk(&mut self, init_stack: Vec<(usize, u16, usize)>, lo: usize, n: usize) {
        debug_assert!(n >= 1);
        let hi = lo + n - 1; // inclusive
        let mut stack = init_stack;

        // Bump `leaf` of every node on the initial (seek) stack if in range.
        // These are the ancestors of `lo` plus `lo`'s owning node.
        for &(phys, _mask, _nib) in &stack {
            let l = self.arena[phys].leaf.get().as_usize();
            if l >= lo && l <= hi {
                self.arena[phys].leaf = OptNz::from_index(PTR::from_usize(l + 1));
            }
        }

        // Walk forward exactly n keys, bumping each leaf-child structural ptr.
        let mut seen = 0;
        while seen < n {
            let &(phys, _mask, nib) = stack.last().expect("bump_walk: stack emptied early");
            if nib == TERMINAL_NIB {
                // Terminal key: its structural ptr is `arena[phys].leaf`, already
                // bumped above (when this frame was pushed — its leaf == this
                // key's index, which is in range).
                seen += 1;
            } else {
                let k = self.arena[phys].children[nib].get().as_usize();
                // k ∈ [lo,hi] by construction (we visit exactly the shifted run).
                self.arena[phys].children[nib] = OptNz::from_index(PTR::from_usize(k + 1));
                seen += 1;
            }
            if seen == n {
                break;
            }
            if !self.bump_advance(&mut stack, lo, hi) {
                debug_assert!(seen >= n, "bump_walk: tree exhausted before n keys");
                break;
            }
        }
    }

    /// `descend_first` with `leaf`-bumping: walk down the lowest-nib spine of
    /// the subtree at `phys`, pushing a frame per node and bumping each node's
    /// `leaf` if in range, until a terminal key or a leaf-child is current.
    fn bump_descend_first(
        &mut self,
        stack: &mut Vec<(usize, u16, usize)>,
        mut phys: usize,
        lo: usize,
        hi: usize,
    ) {
        loop {
            let node = self.arena[phys]; // Copy to avoid borrow conflicts.
            let l = node.leaf.get().as_usize();
            if l >= lo && l <= hi {
                self.arena[phys].leaf = OptNz::from_index(PTR::from_usize(l + 1));
            }
            if node.is_terminal() {
                let mask = node.children_mask();
                stack.push((phys, mask, TERMINAL_NIB));
                return;
            }
            let mask = node.children_mask();
            debug_assert!(mask != 0, "bump_descend_first: non-terminal node with no children");
            let nib = mask.trailing_zeros() as usize;
            stack.push((phys, mask, nib));
            if node.is_leaf(nib) {
                return;
            } else {
                phys = node.children[nib].get().as_usize();
            }
        }
    }

    /// `push_next_child` with descent: find the next occupied nibble ≥
    /// `start_nib` at `encoded`, push its frame, and if it is an internal
    /// child, `bump_descend_first` into it. Returns false if no such nibble.
    #[inline]
    fn bump_push_next(
        &mut self,
        stack: &mut Vec<(usize, u16, usize)>,
        encoded: usize,
        mask: u16,
        start_nib: usize,
        lo: usize,
        hi: usize,
    ) -> bool {
        let shifted = if start_nib >= 16 { 0u16 } else { mask >> start_nib };
        if shifted == 0 {
            return false;
        }
        let nib = start_nib + shifted.trailing_zeros() as usize;
        debug_assert!(nib < 16);
        stack.push((encoded, mask, nib));
        if !self.arena[encoded].is_leaf(nib) {
            let addr = self.arena[encoded].children[nib].get().as_usize();
            self.bump_descend_first(stack, addr, lo, hi);
        }
        true
    }

    /// `advance_next` with mutation: pop frames and `bump_push_next` from the
    /// next nibble until a key is current. Returns false if the stack empties.
    #[inline]
    fn bump_advance(
        &mut self,
        stack: &mut Vec<(usize, u16, usize)>,
        lo: usize,
        hi: usize,
    ) -> bool {
        loop {
            let (encoded, mask, nib) = match stack.pop() {
                Some(v) => v,
                None => return false,
            };
            if nib == TERMINAL_NIB {
                if self.bump_push_next(stack, encoded, mask, 0, lo, hi) {
                    return true;
                }
                continue;
            }
            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };
            if self.bump_push_next(stack, encoded, mask, search_start, lo, hi) {
                return true;
            }
        }
    }

    /// Wire the new key (at slot `p`) into the arena according to `case`, then
    /// propagate the leftmost-`leaf` invariant up the spine. Re-reads any
    /// bump-sensitive leaf key index from the arena (post-bump) instead of using
    /// a value captured before the bump — notably `SplitLeaf`'s existing key,
    /// which may have been the successor `p` and shifted to `p+1`.
    fn execute_case(&mut self, case: Case, p: usize, path: &[(usize, usize)]) {
        let p_idx = PTR::from_usize(p);
        match case {
            Case::Terminal { phys } => {
                self.arena[phys].set_terminal(true);
                self.arena[phys].leaf = OptNz::from_index(p_idx);
                self.up_walk_leftmost(phys, p_idx, path);
            }
            Case::NewLeafChild { phys, nib } => {
                self.arena[phys].set_leaf_child(nib, p_idx);
                self.update_leftmost_on_leaf_insert(phys, nib, p_idx, path);
            }
            Case::SplitNode {
                phys,
                diverge,
                new_is_terminal,
                new_nib,
                ref_nib,
                new_is_leftmost,
            } => {
                let mut new_parent = Node::new();
                new_parent.prefix_len = LEN::from_usize(diverge);
                if new_is_terminal {
                    new_parent.set_terminal(true);
                    new_parent.leaf = OptNz::from_index(p_idx);
                } else {
                    new_parent.set_leaf_child(new_nib, p_idx);
                    if new_is_leftmost {
                        new_parent.leaf = OptNz::from_index(p_idx);
                    }
                }
                let old_node = std::mem::replace(&mut self.arena[phys], new_parent);
                let old_addr = PTR::from_usize(self.arena.len()); // new node index (>= 1)
                self.arena.push(old_node);
                self.arena[phys].set_internal_child(ref_nib, old_addr);
                if new_is_leftmost {
                    // New key is the new parent's leftmost — propagate up the spine.
                    // (Overrides the bump's p→p+1 on this spine back to p.)
                    self.up_walk_leftmost(phys, p_idx, path);
                } else {
                    // Old subtree is leftmost; its leftmost key index lives on in
                    // the pushed old node and was bumped there if in range.
                    self.arena[phys].leaf = self.arena[old_addr.as_usize()].leaf;
                }
            }
            Case::SplitLeaf {
                phys,
                nib,
                d,
                new_is_terminal,
                existing_is_terminal,
                new_nib,
                exist_nib,
                new_is_leftmost,
            } => {
                // Re-read existing key index post-bump (may have been bumped
                // from p to p+1 if existing was the successor).
                let existing_key_index = self.arena[phys].children[nib].get();
                let mut split_node = Node::new();
                split_node.prefix_len = LEN::from_usize(d);
                if new_is_terminal {
                    split_node.set_terminal(true);
                    split_node.leaf = OptNz::from_index(p_idx);
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                } else if existing_is_terminal {
                    split_node.set_terminal(true);
                    split_node.leaf = OptNz::from_index(existing_key_index);
                    split_node.set_leaf_child(new_nib, p_idx);
                } else {
                    split_node.set_leaf_child(new_nib, p_idx);
                    split_node.set_leaf_child(exist_nib, existing_key_index);
                    split_node.leaf = OptNz::from_index(if new_is_leftmost {
                        p_idx
                    } else {
                        existing_key_index
                    });
                }
                let split_addr = PTR::from_usize(self.arena.len());
                self.arena.push(split_node);
                self.arena[phys].set_internal_child(nib, split_addr);
                if new_is_leftmost {
                    // path.last() == (phys, nib): if split_node is phys's leftmost
                    // child, propagate the new leftmost up the spine.
                    self.up_walk_leftmost(split_addr.as_usize(), p_idx, path);
                }
            }
        }
    }

    /// If the new leaf child at `nib` is the lowest occupied nib of `phys_idx`,
    /// it is the node's new leftmost descendant — set `phys_idx.leaf` and
    /// propagate the new leftmost up the leftmost spine via `path`.
    #[inline]
    fn update_leftmost_on_leaf_insert(
        &mut self,
        phys_idx: usize,
        nib: usize,
        new_index: PTR,
        path: &[(usize, usize)],
    ) {
        // A terminal node's own key is a prefix of all its descendants, so it
        // is always the leftmost — a new leaf child can never precede it.
        if self.arena[phys_idx].is_terminal() {
            return;
        }
        let mask = self.arena[phys_idx].children_mask();
        let lowest = mask.trailing_zeros() as usize;
        if nib == lowest {
            self.arena[phys_idx].leaf = OptNz::from_index(new_index);
            self.up_walk_leftmost(phys_idx, new_index, path);
        }
    }

    /// Propagate `new_leftmost` up the leftmost spine: for each ancestor in
    /// `path` (deepest first) via which we descended through that ancestor's
    /// lowest occupied nib, set its `leaf` to `new_leftmost`. Stop at the first
    /// ancestor where the descent was not through its leftmost child, OR at a
    /// terminal ancestor — a terminal node's own key is a prefix of all its
    /// descendants, so it is always that subtree's leftmost and its `leaf` must
    /// stay pinned to the terminal key's slot (ancestors above see that same
    /// fixed leftmost, so propagation stops entirely there).
    #[inline]
    fn up_walk_leftmost(&mut self, attach_phys: usize, new_leftmost: PTR, path: &[(usize, usize)]) {
        let _ = attach_phys; // attach's parent is path.last(); attach itself already set.
        let mut idx = path.len();
        while idx > 0 {
            idx -= 1;
            let (parent_phys, nib) = path[idx];
            if self.arena[parent_phys].is_terminal() {
                break;
            }
            let parent_mask = self.arena[parent_phys].children_mask();
            let lowest = parent_mask.trailing_zeros() as usize;
            if nib == lowest {
                self.arena[parent_phys].leaf = OptNz::from_index(new_leftmost);
            } else {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Insert helpers
    // -----------------------------------------------------------------------

    /// Empty trie: `index` is `[None]` (dummy at 0). Place the key at slot 1
    /// and build a root that is terminal (0-length key) or a leaf child.
    #[inline]
    fn insert_into_empty_trie(
        &mut self,
        off: usize,
        key_len: LEN,
        value: T,
        key: &[u8],
        max_nib: usize,
    ) -> usize {
        let p = 1usize;
        self.index
            .push(Some((NonZero::new(off).unwrap(), key_len, value)));
        let p_idx = PTR::from_usize(p);
        if max_nib == 0 {
            let mut root = Node::new();
            root.set_terminal(true);
            root.leaf = OptNz::from_index(p_idx);
            self.arena.push(root);
        } else {
            let first_nib = key_nibble_at(key, 0) as usize;
            let mut root = Node::new();
            root.set_leaf_child(first_nib, p_idx);
            root.leaf = OptNz::from_index(p_idx);
            self.arena.push(root);
        }
        p
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
            n_keys: self.n_keys,
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
            n_keys: self.n_keys,
            _key: PhantomData,
        })
    }
}


// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

/// Sentinel nib value meaning "positioned at the terminal value of this node."
const TERMINAL_NIB: usize = 16;

/// Internal tree-walking cursor (stack-based arena DFS). Used only for
/// `bump_walk`'s seek-positioning and to land the public `Cursor`'s `seek` on
/// a key in O(keylen). Public iteration uses the linear-scan [`Cursor`].
pub(crate) struct NibbleIter<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> {
    trie: &'a NibbleTrie<K, T, PTR, LEN>,
    /// Stack of (arena_index, occupancy_mask, nibble_position) tuples.
    pub(crate) stack: Vec<(PTR, u16, usize)>,
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
            let (off, len, val) = self.trie.index[ki.as_usize()].as_ref().unwrap();
            let off = off.get();
            let key = &self.trie.buf[off..off + len.as_usize()];
            Some((key, val))
        } else if let Some(key_index) = node.leaf_key_index(*nib) {
            let key = self.trie.key_slice(key_index);
            let val = &self.trie.index[key_index.as_usize()].as_ref().unwrap().2;
            Some((key, val))
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
    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        if self.advance_next() { self.current() } else { None }
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
                let (off, len, _) = self.trie.index[ki.as_usize()].as_ref().unwrap();
                let off = off.get();
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
// Cursor — public linear-scan iterator over the sparse `index`
// ---------------------------------------------------------------------------

/// Public iteration cursor over a [`NibbleTrie`]: a linear scan of the sparse
/// `index`, skipping `None` gaps. This is correct because the index is kept
/// sorted by invariant — occupied slots appear in non-decreasing key order
/// (enforced by the Stage B shift-and-bump insert, and checked by the
/// invariant-oracle tests).
///
/// `iter()` parks *before* the first key (`current()` is `None`, `next()`
/// yields the first key — the idiomatic `Iterator` model); `iter_last()` parks
/// *on* the last key (`current()` returns it, `prev()` walks backward). `seek`
/// lands in O(keylen) via the internal tree walker, then `next`/`prev` resume
/// the linear scan. `first`/`last` jump to the ends. The current key/value is
/// cached at park time, so `current()` (and a `next().current()` follow-up) is
/// a pure field read with no re-scan.
///
/// The cached refs borrow the trie (lifetime `'a`), not the cursor, so values
/// returned by `current`/`next`/`prev`/`seek` outlive the cursor borrow.
pub struct Cursor<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> {
    trie: &'a NibbleTrie<K, T, PTR, LEN>,
    /// Slot index parked on, or a sentinel: `0` = before-first / backward
    /// exhausted (slot 0 is the dummy `None`), `index.len()` = forward
    /// exhausted. A parked `pos` is always a `Some` slot in `[1, len-1]`.
    pos: usize,
    /// Cached `current()` value: `Some` iff `pos` is a `Some` slot.
    cur: Option<(&'a [u8], &'a T)>,
}

impl<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex> Cursor<'a, K, T, PTR, LEN> {
    /// Park at slot `pos`, recomputing the cached current value from `index`.
    /// `pos` may be a sentinel (`0` or `len`) or a `Some` slot.
    fn park(&mut self, pos: usize) {
        self.pos = pos;
        // Read through the `'a` trie ref so the cached refs carry lifetime `'a`
        // (borrowing `*self.trie`, not `*self`) — safe to then write `self.cur`.
        self.cur = match self.trie.index.get(pos) {
            Some(Some(slot)) => {
                let off = slot.0.get();
                let klen = slot.1.as_usize();
                Some((&self.trie.buf[off..off + klen], &slot.2))
            }
            _ => None,
        };
    }

    /// Forward cursor parked *before* the first key.
    pub fn new(trie: &'a NibbleTrie<K, T, PTR, LEN>) -> Self {
        Cursor { trie, pos: 0, cur: None }
    }

    /// Reverse cursor parked *on* the last key (or before-first if empty).
    pub fn new_last(trie: &'a NibbleTrie<K, T, PTR, LEN>) -> Self {
        let mut c = Cursor { trie, pos: 0, cur: None };
        c.last();
        c
    }

    /// Jump to the first key (smallest slot). Returns its key/value, or `None`
    /// if the trie is empty. Scans forward from slot 1.
    pub fn first(&mut self) -> Option<(&'a [u8], &'a T)> {
        let len = self.trie.index.len();
        let mut i = 1;
        while i < len {
            if self.trie.index[i].is_some() {
                self.park(i);
                return self.cur;
            }
            i += 1;
        }
        self.park(0);
        None
    }

    /// Jump to the last key (largest slot). Returns its key/value, or `None` if
    /// the trie is empty. Scans backward from the end of `index`.
    pub fn last(&mut self) -> Option<(&'a [u8], &'a T)> {
        let mut i = self.trie.index.len();
        while i > 1 {
            i -= 1;
            if self.trie.index[i].is_some() {
                self.park(i);
                return self.cur;
            }
        }
        self.park(0);
        None
    }

    /// The key/value the cursor is parked on, or `None` if not parked (before
    /// first, or exhausted). A pure field read — cached by `park`.
    #[inline]
    pub fn current(&self) -> Option<(&'a [u8], &'a T)> {
        self.cur
    }

    /// The slot index the cursor is parked on, or `None` if not parked.
    #[inline]
    pub fn current_index(&self) -> Option<usize> {
        if self.cur.is_some() { Some(self.pos) } else { None }
    }

    /// Advance to the next occupied slot and return its key/value. Returns
    /// `None` (parking at the forward-exhausted sentinel) when no further key
    /// exists.
    #[inline]
    pub fn next(&mut self) -> Option<(&'a [u8], &'a T)> {
        if self.advance_next() { self.cur } else { None }
    }

    /// Step to the previous occupied slot and return its key/value. Returns
    /// `None` (parking at the before-first sentinel) when no prior key exists.
    #[inline]
    pub fn prev(&mut self) -> Option<(&'a [u8], &'a T)> {
        if self.advance_prev() { self.cur } else { None }
    }

    #[inline]
    pub fn next_index(&mut self) -> Option<usize> {
        if self.advance_next() { Some(self.pos) } else { None }
    }

    #[inline]
    pub fn prev_index(&mut self) -> Option<usize> {
        if self.advance_prev() { Some(self.pos) } else { None }
    }

    /// Land on the first key ≥ `key` — O(keylen) via the internal tree walker —
    /// then return its key/value. Returns `None` if no key is ≥ `key`.
    pub fn seek(&mut self, key: &[u8]) -> Option<(&'a [u8], &'a T)> {
        let pos = {
            let mut w = self.trie.walk_iter();
            w.seek(key);
            w.current_index()
        };
        match pos {
            Some(p) => { self.park(p); self.cur }
            None => { self.park(self.trie.index.len()); None }
        }
    }

    // --- core linear scans ---

    /// Scan forward from `pos+1` to the next `Some` slot; park there on hit,
    /// or at the `len` sentinel on miss.
    fn advance_next(&mut self) -> bool {
        let len = self.trie.index.len();
        let mut i = self.pos + 1;
        while i < len {
            if self.trie.index[i].is_some() {
                self.park(i);
                return true;
            }
            i += 1;
        }
        self.park(len);
        false
    }

    /// Scan backward from `pos-1` to the previous `Some` slot; park there on
    /// hit, or at the `0` (before-first) sentinel on miss.
    fn advance_prev(&mut self) -> bool {
        let mut i = self.pos;
        while i > 1 {
            i -= 1;
            if self.trie.index[i].is_some() {
                self.park(i);
                return true;
            }
        }
        self.park(0);
        false
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