//! Nibble Trie — a fixed-fanout radix trie indexed by nibbles (half-bytes).
//!
//! Each node has 16 child slots (one per nibble value 0–15), addressed by
//! direct indexing rather than binary search or SIMD. This trades space for
//! simplicity and lookup speed: no comparison loops, no branch misprediction
//! on the child search path.
//!
//! # Node Stacking (STAK)
//!
//! Multiple "virtual nodes" (vnodes) can share a single physical `Node` in the
//! arena, each claiming a subset of the `children[16]` array via an `occupancy`
//! mask. This is controlled by the `STAK` const generic parameter:
//!
//! - `STAK = 1`: no stacking (each physical node holds one vnode, backward-compatible)
//! - `STAK = 2/4/8`: up to 2/4/8 vnodes per physical node, reducing arena size
//!
//! Child pointers encode both physical node index and vnode index:
//! `address = phys_idx * STAK + vnode_idx`. The sentinel value is `PTR::MAX`.
//!
//! # Terminal Nodes
//!
//! Keys that are prefixes of other keys (e.g. "ab" in {"ab", "abc"}) are
//! represented by a `terminal` flag on the vnode where the key ends, rather
//! than a null-byte leaf child. This eliminates null terminators, allows
//! `0x00` bytes in keys, and makes `get()` accept plain `&[u8]`.
//!
//! # Key Index Encoding
//!
//! Real keys start at index 1 (index 0 is the dummy entry). This allows
//! `PTR::MAX` to be used as sentinel for empty slots in `children[]` when
//! STAK > 1. For STAK = 1, `PTR::zero()` is also unused as a child index
//! (root is at phys 0, vnode 0, encoded as address 0, and is never a child
//! of another node). The `occupancy[v]` mask determines whether a slot is
//! owned by vnode v, regardless of the value in `children[nib]`.

use crate::{ByteKey, TinyTrieMap};
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
    /// Zero value, used for initial values.
    fn zero() -> Self;
    /// Maximum value used as sentinel for empty slots in `children[]`.
    /// With stacking, encoded addresses use 0 as a valid address (root = phys 0, vnode 0),
    /// so `PTR::MAX` is the sentinel instead of 0.
    fn max_value_sentinel() -> Self;
    /// Convert from `usize`. May panic or truncate on overflow in debug builds.
    fn from_usize(n: usize) -> Self;
    /// Compute a 16-bit occupancy mask from a 16-slot children array.
    /// Bit N is set if `children[N]` is not the sentinel value.
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
// Core types
// ---------------------------------------------------------------------------

/// A single node in the nibble trie arena.
///
/// Generic over `PTR` (pointer/index type for children and arena references),
/// `LEN` (length type for prefix lengths and key lengths), and `STAK` (maximum
/// number of virtual nodes per physical node, must be a power of 2 ≤ 8).
///
/// With stacking, each physical node can hold up to `STAK` virtual nodes (vnodes).
/// Each vnode has its own `prefix_len`, `leaf_mask`, `occupancy`, `leaf`, and
/// terminal bit, but they share the `children[16]` array. The `occupancy[v]`
/// mask determines which nibble slots belong to vnode `v`.
///
/// Child pointers encode both physical node index and vnode index:
/// `address = phys_idx * STAK + vnode_idx`. Decode with `address / STAK` and
/// `address % STAK`. The sentinel value `PTR::MAX` marks empty slots.
///
/// Layout with PTR=u32, LEN=u16:
/// - STAK=1: 76 bytes (4B smaller than pre-stacking 80-byte node)
/// - STAK=2: 88 bytes (saves 72B vs 2×80, 45%)
/// - STAK=4: 108 bytes (saves 212B vs 4×80, 66%)
#[derive(Copy, Clone)]
pub struct Node<PTR: TrieIndex, LEN: TrieIndex, const STAK: usize = 1> {
    pub children: [PTR; 16],     // shared across all vnodes; PTR::MAX = empty
    pub leaf: PTR,               // key index for prefix comparison (shared by all vnodes)
    pub prefix_len: [LEN; STAK], // per-vnode prefix length (must be increasing)
    pub leaf_mask: [u16; STAK],  // per-vnode: bit N = children[N] is leaf key index
    pub occupancy: [u16; STAK],  // per-vnode: bit N = this vnode owns slot N
    pub nodelen: u8,             // number of vnodes in this physical node (1..=STAK)
    pub terminal: u8,            // bitmask: bit V = vnode V is terminal
}

impl<PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> Node<PTR, LEN, STAK> {
    pub fn new() -> Self {
        Node {
            children: [PTR::max_value_sentinel(); 16],
            leaf: PTR::max_value_sentinel(),
            prefix_len: [LEN::zero(); STAK],
            leaf_mask: [0; STAK],
            occupancy: [0; STAK],
            nodelen: 1, // new physical node starts with 1 vnode
            terminal: 0,
        }
    }

    /// Check if vnode `v` is terminal.
    #[inline]
    pub fn is_terminal(&self, v: usize) -> bool {
        debug_assert!(v < STAK, "vnode index {v} >= STAK {STAK}");
        (self.terminal >> v) & 1 == 1
    }

    /// Set terminal flag for vnode `v`.
    #[inline]
    fn set_terminal(&mut self, v: usize, val: bool) {
        debug_assert!(v < STAK);
        if val {
            self.terminal |= 1 << v;
        } else {
            self.terminal &= !(1 << v);
        }
    }

    /// Check if nibble slot `nib` in vnode `v` is a leaf (key index).
    #[inline]
    pub fn is_leaf(&self, nib: usize, v: usize) -> bool {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        (self.leaf_mask[v] >> nib) & 1 == 1
    }

    /// Set the leaf flag for nibble slot `nib` in vnode `v`.
    #[inline]
    fn set_leaf(&mut self, nib: usize, v: usize) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        self.leaf_mask[v] |= 1 << nib;
    }

    /// Clear the leaf flag for nibble slot `nib` in vnode `v`.
    #[inline]
    fn clear_leaf(&mut self, nib: usize, v: usize) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        self.leaf_mask[v] &= !(1 << nib);
    }

    /// Check if nibble slot `nib` is owned by vnode `v`.
    #[inline]
    pub fn is_occupied(&self, nib: usize, v: usize) -> bool {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        (self.occupancy[v] >> nib) & 1 == 1
    }

    /// Set the occupancy bit for nibble slot `nib` in vnode `v`.
    #[inline]
    fn set_occupied(&mut self, nib: usize, v: usize) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        self.occupancy[v] |= 1 << nib;
    }

    /// Clear the occupancy bit for nibble slot `nib` in vnode `v`.
    #[inline]
    #[allow(dead_code)]
    fn clear_occupied(&mut self, nib: usize, v: usize) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        self.occupancy[v] &= !(1 << nib);
    }

    /// Store a leaf key index at `nib` in vnode `v`.
    /// Key index must not be the sentinel value.
    #[inline]
    fn set_leaf_child(&mut self, nib: usize, v: usize, key_index: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        debug_assert!(key_index != PTR::max_value_sentinel(), "sentinel key index");
        self.set_leaf(nib, v);
        self.set_occupied(nib, v);
        self.children[nib] = key_index;
    }

    /// Store an encoded address at `nib` in vnode `v` (internal node reference).
    #[inline]
    fn set_internal_child(&mut self, nib: usize, v: usize, addr: PTR) {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        debug_assert!(addr != PTR::max_value_sentinel(), "sentinel address");
        self.clear_leaf(nib, v);
        self.set_occupied(nib, v);
        self.children[nib] = addr;
    }

    /// Decode a leaf child at `nib` in vnode `v` into a key index.
    /// Returns `None` if the slot is empty or not a leaf.
    #[inline]
    fn leaf_key_index(&self, nib: usize, v: usize) -> Option<PTR> {
        debug_assert!(nib < 16);
        debug_assert!(v < STAK);
        if self.is_leaf(nib, v) && self.is_occupied(nib, v) {
            Some(self.children[nib])
        } else {
            None
        }
    }

    /// Compute a 16-bit mask where bit N is set if `children[N]` is not the sentinel.
    /// For STAK=1, this equals `occupancy[0]` which is maintained on every mutation.
    /// For STAK>1, this returns the merged occupancy across all vnodes.
    #[inline]
    pub fn children_mask(&self) -> u16 {
        self.merged_occupancy()
    }

    /// Compute the merged occupancy mask: OR of all vnodes' occupancy masks.
    #[inline]
    pub fn merged_occupancy(&self) -> u16 {
        let mut merged = 0u16;
        for v in 0..STAK {
            merged |= self.occupancy[v];
        }
        merged
    }

    /// Return the number of vnodes in this physical node (1..=STAK).
    /// This is maintained by insert (always 1) and optimize (increments when stacking).
    #[inline]
    pub fn vnode_count(&self) -> usize {
        self.nodelen as usize
    }

    /// Encode a physical node index and vnode index into an address.
    #[inline]
    pub fn encode_addr(phys_idx: usize, vnode_idx: usize) -> PTR {
        PTR::from_usize(phys_idx * STAK + vnode_idx)
    }

    /// Decode the physical node index from an address.
    #[inline]
    pub fn decode_phys(addr: PTR) -> usize {
        addr.as_usize() / STAK
    }

    /// Decode the vnode index from an address.
    #[inline]
    pub fn decode_vnode(addr: PTR) -> usize {
        addr.as_usize() % STAK
    }

    /// Promote this node's PTR type to a wider one, preserving STAK.
    /// Child addresses and leaf indices are widened via `NewPTR::from_usize`.
    /// Since STAK is unchanged, no address remapping is needed.
    pub fn promote<NewPTR: TrieIndex>(self) -> Node<NewPTR, LEN, STAK> {
        let merged = self.merged_occupancy();
        let mut children = [NewPTR::max_value_sentinel(); 16];
        for i in 0..16 {
            if merged & (1 << i) != 0 {
                children[i] = NewPTR::from_usize(self.children[i].as_usize());
            }
        }
        Node {
            children,
            leaf: NewPTR::from_usize(self.leaf.as_usize()),
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            occupancy: self.occupancy,
            nodelen: self.nodelen,
            terminal: self.terminal,
        }
    }

    /// Demote this node's PTR type to a narrower one, preserving STAK.
    /// Returns `Err(self)` if any child address or leaf index doesn't fit
    /// in the narrower type.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<Node<NewPTR, LEN, STAK>, Self> {
        let merged = self.merged_occupancy();
        for i in 0..16 {
            if merged & (1 << i) != 0 {
                if self.children[i].as_usize() > NewPTR::max_value() {
                    return Err(self);
                }
            }
        }
        if self.leaf != PTR::max_value_sentinel() && self.leaf.as_usize() > NewPTR::max_value() {
            return Err(self);
        }
        let mut children = [NewPTR::max_value_sentinel(); 16];
        for i in 0..16 {
            if merged & (1 << i) != 0 {
                children[i] = NewPTR::from_usize(self.children[i].as_usize());
            }
        }
        Ok(Node {
            children,
            leaf: if self.leaf == PTR::max_value_sentinel() {
                NewPTR::max_value_sentinel()
            } else {
                NewPTR::from_usize(self.leaf.as_usize())
            },
            prefix_len: self.prefix_len,
            leaf_mask: self.leaf_mask,
            occupancy: self.occupancy,
            nodelen: self.nodelen,
            terminal: self.terminal,
        })
    }
}

impl<PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> fmt::Debug for Node<PTR, LEN, STAK> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active: Vec<(usize, &str, PTR)> = (0..16)
            .filter(|&n| self.children[n] != PTR::max_value_sentinel())
            .map(|n| {
                let tag = if self.is_leaf(n, 0) { "L" } else { "I" };
                (n, tag, self.children[n])
            })
            .collect();
        f.debug_struct("Node")
            .field("prefix_len", &self.prefix_len)
            .field("leaf_mask", &format_args!("{:?}", self.leaf_mask))
            .field("occupancy", &format_args!("{:?}", self.occupancy))
            .field("terminal", &format_args!("{:08b}", self.terminal))
            .field("leaf", &self.leaf)
            .field("children", &active)
            .finish()
    }
}

/// Entry on the parent stack during optimize stacking decisions.
struct StackEntry {
    _old_phys: usize,
    new_phys_idx: usize,
    _vnode: usize,
}

// ---------------------------------------------------------------------------
// NibbleTrie
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NibbleTrie<K, T, PTR: TrieIndex = u32, LEN: TrieIndex = u16, const STAK: usize = 1>
where
    K: ByteKey,
{
    pub arena: Vec<Node<PTR, LEN, STAK>>,
    pub buf: Vec<u8>,                // all keys concatenated (no null terminators)
    pub index: Vec<(usize, LEN)>,    // (offset into buf, len) per key — offset is usize, len is compact
    pub values: Vec<T>,              // values[i] ↔ index[i]
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
// NibbleTrie methods (generic over STAK)
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> NibbleTrie<K, T, PTR, LEN, STAK> {
    /// Return the key slice for `key_index`.
    #[inline]
    fn key_slice(&self, key_index: PTR) -> &[u8] {
        let (off, len) = self.index[key_index.as_usize()];
        &self.buf[off..off + len.as_usize()]
    }

    /// Unchecked version of `key_slice` — skips bounds checks on index and buf.
    #[inline]
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
    // Lookup (generic over STAK)
    // -----------------------------------------------------------------------

    pub fn get(&self, key: &[u8]) -> Option<usize> {
        if self.arena.is_empty() {
            return None;
        }
        let mut phys_idx: usize = 0;
        let mut vnode_idx: usize = 0;
        let max_nib = key.len() * 2;
        loop {
            let node = &self.arena[phys_idx];
            let prefix_len = node.prefix_len[vnode_idx].as_usize();
            // Key nibbles exhausted — check if this vnode is terminal.
            // The leaf key may be longer than the search key (from a deeper vnode
            // sharing the physical node), so we compare only up to the search key's
            // length. All nibbles up to prefix_len have been verified during descent.
            if prefix_len >= max_nib {
                if node.is_terminal(vnode_idx) {
                    let ki = node.leaf;
                    let (off, len) = self.index[ki.as_usize()];
                    let key_in_buf = &self.buf[off..off + len.as_usize()];
                    if key.len() == len.as_usize() && simd_eq(&key_in_buf[..key.len()], key) {
                        return Some(ki.as_usize());
                    }
                }
                return None;
            }
            let nib = key_nibble_at(key, prefix_len) as usize;
            if !node.is_occupied(nib, vnode_idx) {
                return None;
            }
            let slot = node.children[nib];
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if node.is_leaf(nib, vnode_idx) {
                let key_index = slot;
                return if simd_eq(self.key_slice(key_index), key) {
                    Some(key_index.as_usize())
                } else {
                    None
                };
            }
            // Internal child — decode address
            phys_idx = slot.as_usize() / STAK;
            vnode_idx = slot.as_usize() % STAK;
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
        let mut vnode_idx: usize = 0;
        let max_nib = key.len() * 2;
        loop {
            let node = unsafe { self.arena.get_unchecked(phys_idx) };
            let prefix_len = node.prefix_len[vnode_idx].as_usize();
            if prefix_len >= max_nib {
                debug_assert!(node.is_terminal(vnode_idx), "get_unchecked: key not in set");
                return Some(node.leaf.as_usize());
            }
            let nib = unsafe { key_nibble_at_unchecked(key, prefix_len) } as usize;
            let slot = unsafe { *node.children.get_unchecked(nib) };
            if slot == PTR::max_value_sentinel() {
                return None;
            }
            if node.is_leaf(nib, vnode_idx) {
                return Some(slot.as_usize());
            }
            phys_idx = slot.as_usize() / STAK;
            vnode_idx = slot.as_usize() % STAK;
        }
    }

    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        self.get(key).map(|idx| &self.values[idx - 1])
    }

    // -----------------------------------------------------------------------
    // Iteration
    // -----------------------------------------------------------------------

    pub fn iter(&self) -> NibbleIter<'_, K, T, PTR, LEN, STAK> {
        NibbleIter::new(self)
    }

    pub fn iter_last(&self) -> NibbleIter<'_, K, T, PTR, LEN, STAK> {
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
        // For STAK > 1, encoded addresses are phys_idx * STAK + vnode_idx.
        // We need arena.len() * STAK < PTR::max_value() for valid encoded addresses.
        // Also need index.len() < PTR::max_value() for key indices.
        self.arena.len() >= PTR::max_value() / STAK || self.index.len() >= PTR::max_value()
    }

    // -----------------------------------------------------------------------
    // Optimize (DFS key-sorted buf rewrite + vnode stacking)
    // -----------------------------------------------------------------------

    /// Rewrite `buf` in DFS order and, when STAK > 1, pack ancestor-descendant
    /// vnodes into shared physical nodes to reduce arena size.
    ///
    /// For STAK = 1 this is equivalent to the old buf-only optimize (no stacking
    /// is possible since each physical node holds exactly one vnode).
    pub fn optimize(&mut self) {
        if self.arena.is_empty() {
            return;
        }

        let mut new_buf = vec![0u8; self.buf.len()];
        let mut cursor: usize = 1; // position 0 is the dummy byte

        // Remap table: maps old encoded address → (new_phys, new_vnode).
        // Initialized with (0, 0) as placeholder for index 0.
        // For STAK=1 old addresses are just phys_idx, so remap[phys] gives the
        // new location. For STAK>1 old addresses are phys*STAK+vnode.
        let old_node_count = self.arena.len() * STAK; // upper bound on old vnode count
        let mut remap: Vec<(usize, usize)> = vec![(0, 0); old_node_count];

        // Parent stack for stacking decisions
        let mut parent_stack: Vec<StackEntry> = Vec::new();

        // New arena (rebuilt with stacking)
        let mut new_arena: Vec<Node<PTR, LEN, STAK>> = Vec::new();

        // Collect key indices in DFS visitation order for index/values sorting
        let mut dfs_key_order: Vec<PTR> = Vec::new();

        self.walk_optimize_stacked(
            0, 0, // root at phys=0, vnode=0
            &mut new_buf, &mut cursor,
            &mut remap, &mut parent_stack, &mut new_arena,
            &mut dfs_key_order,
        );

        new_buf.truncate(cursor);
        self.buf = new_buf;
        self.arena = new_arena;

        // Remap all internal child addresses in the new arena
        for phys in 0..self.arena.len() {
            for v in 0..STAK {
                let occ = self.arena[phys].occupancy[v];
                if occ == 0 && !self.arena[phys].is_terminal(v) {
                    continue;
                }
                for nib in 0..16 {
                    if (occ >> nib) & 1 == 0 {
                        continue;
                    }
                    if self.arena[phys].is_leaf(nib, v) {
                        continue; // leaf — value is a key index, not an arena address
                    }
                    let old_addr = self.arena[phys].children[nib].as_usize();
                    debug_assert!(old_addr < remap.len(), "old_addr {} >= remap.len() {}, phys={} v={} nib={}", old_addr, remap.len(), phys, v, nib);
                    debug_assert!(!(remap[old_addr] == (0, 0) && old_addr != 0), "remap[{}] == (0,0) but old_addr != 0, phys={} v={} nib={}", old_addr, phys, v, nib);
                    let (new_phys, new_vnode) = remap[old_addr];
                    self.arena[phys].children[nib] = PTR::from_usize(new_phys * STAK + new_vnode);
                }
            }
        }

        // --- Sort index and values into DFS order ---

        // Build key remap: old key index → new key index (1-based DFS rank).
        // Index 0 is the dummy entry and stays in place.
        let num_keys = dfs_key_order.len();
        let mut key_remap: Vec<usize> = vec![0; self.index.len()];
        key_remap[0] = 0; // dummy stays at 0
        for (new_ki, &old_ki) in dfs_key_order.iter().enumerate() {
            key_remap[old_ki.as_usize()] = new_ki + 1; // 1-based
        }

        // Remap all key index references in the arena
        for phys in 0..self.arena.len() {
            for v in 0..STAK {
                let occ = self.arena[phys].occupancy[v];
                if occ == 0 && !self.arena[phys].is_terminal(v) {
                    continue; // inactive vnode, skip
                }
                // Remap leaf children
                for nib in 0..16 {
                    if (occ >> nib) & 1 == 0 {
                        continue;
                    }
                    if self.arena[phys].is_leaf(nib, v) {
                        let old_ki = self.arena[phys].children[nib].as_usize();
                        let new_ki = key_remap[old_ki];
                        self.arena[phys].children[nib] = PTR::from_usize(new_ki);
                    }
                }
            }
            // Remap leaf pointer (skip sentinel values)
            let old_leaf = self.arena[phys].leaf;
            if old_leaf != PTR::max_value_sentinel() {
                let new_ki = key_remap[old_leaf.as_usize()];
                self.arena[phys].leaf = PTR::from_usize(new_ki);
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

    fn walk_optimize_stacked(
        &mut self,
        old_phys: usize,
        old_vnode: usize,
        new_buf: &mut [u8],
        cursor: &mut usize,
        remap: &mut Vec<(usize, usize)>,
        parent_stack: &mut Vec<StackEntry>,
        new_arena: &mut Vec<Node<PTR, LEN, STAK>>,
        dfs_key_order: &mut Vec<PTR>,
    ) {
        let node = self.arena[old_phys]; // copy to avoid borrow conflicts
        let occ = node.occupancy[old_vnode];
        let is_term = node.is_terminal(old_vnode);

        // --- Decide where this vnode goes in the new arena ---

        // Try to stack into an ancestor (deepest first)
        let mut new_phys: usize = 0;
        let mut assigned_vnode: usize = 0;

        if STAK > 1 {
            // Walk parent stack from deepest (top) to shallowest (bottom)
            // Use the physical node's actual nodelen and merged_occupancy rather than
            // the stack entry's copies, because multiple stack entries may reference
            // the same physical node (from different subtree visits).
            let mut stacked = false;
            for i in (0..parent_stack.len()).rev() {
                let host_phys = parent_stack[i].new_phys_idx;
                let host_nodelen = new_arena[host_phys].nodelen as usize;
                if host_nodelen < STAK {
                    let host_merged = new_arena[host_phys].merged_occupancy();
                    if host_merged & occ == 0 {
                        // Terminal constraint: a pnode with a terminal vnode is
                        // closed for further stacking. This ensures the single
                        // `leaf` field holds the terminal key index, and that
                        // key is long enough for all vnodes' prefix comparisons.
                        if new_arena[host_phys].terminal != 0 {
                            continue;
                        }
                        // Can stack here!
                        new_phys = host_phys;
                        assigned_vnode = host_nodelen; // next available vnode slot
                        new_arena[host_phys].nodelen += 1;
                        stacked = true;
                        break;
                    }
                }
            }
            if !stacked {
                // No ancestor can host — create a new physical node
                new_phys = new_arena.len();
                assigned_vnode = 0;
                new_arena.push(Node::new());
            }
        } else {
            // STAK=1: every vnode gets its own physical node
            new_phys = new_arena.len();
            assigned_vnode = 0;
            new_arena.push(Node::new());
        }

        // Record the remapping
        let old_addr = old_phys * STAK + old_vnode;
        if old_addr < remap.len() {
            remap[old_addr] = (new_phys, assigned_vnode);
        }

        // --- Populate vnode fields ---

        new_arena[new_phys].prefix_len[assigned_vnode] = node.prefix_len[old_vnode];
        new_arena[new_phys].occupancy[assigned_vnode] = occ;
        new_arena[new_phys].leaf_mask[assigned_vnode] = node.leaf_mask[old_vnode];
        if is_term {
            new_arena[new_phys].set_terminal(assigned_vnode, true);
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
            new_arena[new_phys].leaf = ki;
            dfs_key_order.push(ki);
        }

        // --- Recurse into children ---

        // Only push onto parent_stack if this vnode is the last (youngest) one in
        // its old physical node AND the new pnode is not terminal. This enforces:
        // 1. The stacking invariant: only descendants of the last vnode can stack in.
        // 2. The terminal constraint: once a pnode has a terminal vnode, it's closed
        //    for further stacking (the single `leaf` field holds the terminal key).
        let is_last_vnode = old_vnode == node.nodelen as usize - 1;
        let new_pnode_has_terminal = new_arena[new_phys].terminal != 0;
        if is_last_vnode && !new_pnode_has_terminal {
            let stack_entry = StackEntry {
                _old_phys: old_phys,
                new_phys_idx: new_phys,
                _vnode: assigned_vnode,
            };
            parent_stack.push(stack_entry);
        }

        for nib in 0..16 {
            if (occ >> nib) & 1 == 0 {
                continue;
            }
            if node.is_leaf(nib, old_vnode) {
                // Leaf child — copy key data
                let ki = node.children[nib];
                let (old_off, len) = self.index[ki.as_usize()];
                let start = *cursor;
                new_buf[start..start + len.as_usize()].copy_from_slice(
                    &self.buf[old_off..old_off + len.as_usize()]
                );
                self.index[ki.as_usize()].0 = *cursor;
                *cursor += len.as_usize();
                // Store leaf key index in the new arena
                new_arena[new_phys].children[nib] = ki;
                dfs_key_order.push(ki);
            } else {
                // Internal child — recurse, then store the old address for remapping
                let child_old_addr = node.children[nib];
                let child_phys = child_old_addr.as_usize() / STAK;
                let child_vnode = child_old_addr.as_usize() % STAK;
                self.walk_optimize_stacked(
                    child_phys, child_vnode,
                    new_buf, cursor,
                    remap, parent_stack, new_arena,
                    dfs_key_order,
                );
                // Store the old address so the remap loop can find it later
                new_arena[new_phys].children[nib] = child_old_addr;
            }
        }

        // Propagate leaf for non-terminal vnodes: set leaf to the first descendant's
        // key index. For terminal vnodes, leaf was already set during the walk.
        // After recursing into children, their leaves are valid, so we can follow
        // the first child to get the leaf.
        // IMPORTANT: don't overwrite leaf if it was already set by a terminal vnode
        // that was stacked into this pnode (their leaf is authoritative).
        if !is_term && new_arena[new_phys].leaf == PTR::max_value_sentinel() {
            let first_nib = occ.trailing_zeros() as usize;
            if new_arena[new_phys].is_leaf(first_nib, assigned_vnode) {
                new_arena[new_phys].leaf = new_arena[new_phys].children[first_nib];
            } else {
                let child_old_addr = node.children[first_nib];
                if child_old_addr.as_usize() < remap.len() {
                    let child_phys_new = remap[child_old_addr.as_usize()].0;
                    new_arena[new_phys].leaf = new_arena[child_phys_new].leaf;
                }
            }
        }

        if is_last_vnode && !new_pnode_has_terminal {
            parent_stack.pop();
        }
    }
}

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> Default for NibbleTrie<K, T, PTR, LEN, STAK> {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// NibbleTrie implementation (STAK=1, insert-compatible)
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> NibbleTrie<K, T, PTR, LEN, STAK> {
    // -----------------------------------------------------------------------
    // Insertion
    // -----------------------------------------------------------------------

    pub fn insert(&mut self, key: K, value: T) -> Result<usize, ()> {
        let key_bytes = key.as_bytes();
        // Overflow checks: encoded addresses must fit in PTR, and key indices
        // must not produce the sentinel value.
        if self.arena.len() >= PTR::max_value() / STAK || self.index.len() >= PTR::max_value() {
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
            return Ok(self.insert_into_empty_trie(key_bytes, new_index, offset, max_nib));
        }

        let mut phys_idx: usize = 0;
        let mut vnode_idx: usize = 0;
        let mut confirmed: usize = 0;

        loop {
            let node = &self.arena[phys_idx];
            let ki = node.leaf;
            let (off, ref_len) = self.index[ki.as_usize()];
            let ref_key = &self.buf[off..off + ref_len.as_usize()];
            let prefix_len = node.prefix_len[vnode_idx].as_usize();

            match simd_check_prefix::<8>(key_bytes, ref_key, confirmed, prefix_len) {
                PrefixCheck::Diverges(diverge) => {
                    return Ok(self.split_node_before_prefix(
                        phys_idx, vnode_idx, diverge, new_index, offset, key_bytes, max_nib,
                    ));
                }
                PrefixCheck::Matches => {
                    if max_nib == prefix_len {
                        if key_bytes.len() == ref_key.len() {
                            self.rollback_last_insert();
                            return Err(());
                        }
                        self.arena[phys_idx].set_terminal(vnode_idx, true);
                        self.arena[phys_idx].leaf = new_index;
                        return Ok(new_index.as_usize());
                    }

                    confirmed = prefix_len + 1;
                    let nib = key_nibble_at(key_bytes, prefix_len) as usize;
                    if !node.is_occupied(nib, vnode_idx) {
                        // Empty slot — new key diverges here
                        self.arena[phys_idx].set_leaf_child(nib, vnode_idx, new_index);
                        return Ok(new_index.as_usize());
                    }
                    let slot = node.children[nib];

                    if node.is_leaf(nib, vnode_idx) {
                        return self.split_leaf_child(
                            nib, phys_idx, vnode_idx, slot, new_index, offset, key_bytes, max_nib, confirmed,
                        );
                    }

                    phys_idx = slot.as_usize() / STAK;
                    vnode_idx = slot.as_usize() % STAK;
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
    fn insert_into_empty_trie(&mut self, key: &[u8], new_index: PTR, _offset: usize, max_nib: usize) -> usize {
        if max_nib == 0 {
            let mut root = Node::new();
            root.set_terminal(0, true);
            root.leaf = new_index;
            root.prefix_len[0] = LEN::zero();
            self.arena.push(root);
            return new_index.as_usize();
        }
        let first_nib = key_nibble_at(key, 0) as usize;
        let mut root = Node::new();
        root.set_leaf_child(first_nib, 0, new_index);
        root.leaf = new_index;
        root.prefix_len[0] = LEN::zero();
        self.arena.push(root);
        new_index.as_usize()
    }

    #[inline]
    fn split_node_before_prefix(
        &mut self,
        phys_idx: usize,
        vnode_idx: usize,
        diverge: usize,
        new_index: PTR,
        _offset: usize,
        key: &[u8],
        max_nib: usize,
    ) -> usize {
        let node = &self.arena[phys_idx];
        let ki = node.leaf;
        let (off, ref_len) = self.index[ki.as_usize()];
        let ref_key = &self.buf[off..off + ref_len.as_usize()];

        let new_nib = key_nibble_at(key, diverge) as usize;
        let ref_nib = key_nibble_at(ref_key, diverge) as usize;

        let mut new_parent = Node::new();
        new_parent.prefix_len[0] = LEN::from_usize(diverge);

        if diverge >= max_nib {
            new_parent.set_terminal(0, true);
            new_parent.leaf = new_index;
        } else {
            new_parent.set_leaf_child(new_nib, 0, new_index);
            new_parent.leaf = new_index;
        }

        let old_node = std::mem::replace(&mut self.arena[phys_idx], new_parent);
        let old_addr = PTR::from_usize(self.arena.len() * STAK); // new node at vnode 0
        self.arena.push(old_node);

        self.arena[phys_idx].set_internal_child(ref_nib, vnode_idx, old_addr);

        new_index.as_usize()
    }

    #[inline]
    fn split_leaf_child(
        &mut self,
        nib: usize,
        phys_idx: usize,
        vnode_idx: usize,
        existing_key_index: PTR,
        new_index: PTR,
        _offset: usize,
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
                split_node.prefix_len[0] = LEN::from_usize(d);

                if d >= max_nib {
                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                    split_node.set_terminal(0, true);
                    split_node.leaf = new_index;
                    split_node.set_leaf_child(exist_nib, 0, existing_key_index);
                } else if d >= existing_key.len() * 2 {
                    let new_nib = key_nibble_at(key, d) as usize;
                    split_node.set_terminal(0, true);
                    split_node.leaf = existing_key_index;
                    split_node.set_leaf_child(new_nib, 0, new_index);
                } else {
                    let new_nib = key_nibble_at(key, d) as usize;
                    let exist_nib = key_nibble_at(existing_key, d) as usize;
                    debug_assert_ne!(new_nib, exist_nib);
                    split_node.set_leaf_child(new_nib, 0, new_index);
                    split_node.set_leaf_child(exist_nib, 0, existing_key_index);
                    split_node.leaf = existing_key_index;
                }

                let split_addr = PTR::from_usize(self.arena.len() * STAK);
                self.arena.push(split_node);
                self.arena[phys_idx].set_internal_child(nib, vnode_idx, split_addr);

                Ok(new_index.as_usize())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PTR width conversions (promote/demote), generic over STAK
// ---------------------------------------------------------------------------

impl<K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> NibbleTrie<K, T, PTR, LEN, STAK> {
    /// Promote the arena index type to a wider PTR, preserving STAK.
    /// All child addresses and leaf indices are widened via `NewPTR::from_usize`.
    /// Since STAK is unchanged, no address remapping is needed.
    pub fn promote<NewPTR: TrieIndex>(self) -> NibbleTrie<K, T, NewPTR, LEN, STAK> {
        let arena = self.arena.into_iter().map(|node| node.promote()).collect();
        NibbleTrie {
            arena,
            buf: self.buf,
            index: self.index,
            values: self.values,
            _key: PhantomData,
        }
    }

    /// Demote the arena index type to a narrower PTR, preserving STAK.
    /// Returns `Err(self)` if any address or index doesn't fit in the narrower type.
    pub fn demote<NewPTR: TrieIndex>(self) -> Result<NibbleTrie<K, T, NewPTR, LEN, STAK>, Self> {
        if self.arena.len() * STAK > NewPTR::max_value() || self.index.len() > NewPTR::max_value() {
            return Err(self);
        }
        // Check each node for overflow before consuming self
        for node in &self.arena {
            if let Err(node) = node.demote::<NewPTR>() {
                // Reconstruct is impossible since we only have the error node,
                // but the capacity check above should catch all cases.
                // Fall through — the per-node check below handles it.
                let _ = node; // suppress unused warning
            }
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

pub struct NibbleIter<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize = 1> {
    trie: &'a NibbleTrie<K, T, PTR, LEN, STAK>,
    /// Stack of (encoded_addr, occupancy_mask, nibble_position, vnode_idx) tuples.
    stack: Vec<(PTR, u16, usize, usize)>,
}

impl<'a, K: ByteKey, T, PTR: TrieIndex, LEN: TrieIndex, const STAK: usize> NibbleIter<'a, K, T, PTR, LEN, STAK> {
    fn new(trie: &'a NibbleTrie<K, T, PTR, LEN, STAK>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mask = trie.arena[0].occupancy[0];
        let nib = if trie.arena[0].is_terminal(0) { TERMINAL_NIB } else { usize::MAX };
        NibbleIter { trie, stack: vec![(PTR::zero(), mask, nib, 0)] }
    }

    fn new_last(trie: &'a NibbleTrie<K, T, PTR, LEN, STAK>) -> Self {
        if trie.arena.is_empty() {
            return NibbleIter { trie, stack: Vec::new() };
        }
        let mut stack = Vec::new();
        let mut phys_idx: usize = 0;
        let mut vnode_idx: usize = 0;
        loop {
            let node = &trie.arena[phys_idx];
            let mask = node.occupancy[vnode_idx];
            if mask != 0 {
                let nib = 15 - mask.leading_zeros() as usize;
                let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                stack.push((encoded, mask, nib, vnode_idx));
                if node.is_leaf(nib, vnode_idx) {
                    break;
                } else {
                    let addr = node.children[nib].as_usize();
                    phys_idx = addr / STAK;
                    vnode_idx = addr % STAK;
                }
            } else if node.is_terminal(vnode_idx) {
                let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                break;
            } else {
                break;
            }
        }
        NibbleIter { trie, stack }
    }

    fn descend_first(&mut self, mut phys_idx: usize, mut vnode_idx: usize) {
        loop {
            let node = &self.trie.arena[phys_idx];
            if node.is_terminal(vnode_idx) {
                let mask = node.occupancy[vnode_idx];
                let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                return;
            }
            let mask = node.occupancy[vnode_idx];
            debug_assert!(mask != 0, "descend_first: non-terminal vnode with no children");
            let nib = mask.trailing_zeros() as usize;
            let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
            self.stack.push((encoded, mask, nib, vnode_idx));
            if node.is_leaf(nib, vnode_idx) {
                return;
            } else {
                let addr = node.children[nib].as_usize();
                phys_idx = addr / STAK;
                vnode_idx = addr % STAK;
            }
        }
    }

    fn descend_last(&mut self, mut phys_idx: usize, mut vnode_idx: usize) {
        loop {
            let node = &self.trie.arena[phys_idx];
            if node.is_terminal(vnode_idx) {
                let mask = node.occupancy[vnode_idx];
                if mask == 0 {
                    let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                    self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                    return;
                }
            }
            let mask = node.occupancy[vnode_idx];
            if mask == 0 {
                if node.is_terminal(vnode_idx) {
                    let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                    self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                }
                return;
            }
            let nib = 15 - mask.leading_zeros() as usize;
            let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
            self.stack.push((encoded, mask, nib, vnode_idx));
            if node.is_leaf(nib, vnode_idx) {
                return;
            } else {
                let addr = node.children[nib].as_usize();
                phys_idx = addr / STAK;
                vnode_idx = addr % STAK;
            }
        }
    }

    #[inline]
    fn push_next_child(&mut self, encoded: PTR, mask: u16, start_nib: usize, vnode_idx: usize) -> bool {
        let shifted = if start_nib >= 16 { 0u16 } else { mask >> start_nib };
        if shifted == 0 {
            return false;
        }
        let nib = start_nib + shifted.trailing_zeros() as usize;
        debug_assert!(nib < 16);
        debug_assert!(mask & (1 << nib) != 0);
        self.stack.push((encoded, mask, nib, vnode_idx));
        let phys_idx = encoded.as_usize() / STAK;
        if !self.trie.arena[phys_idx].is_leaf(nib, vnode_idx) {
            let addr = self.trie.arena[phys_idx].children[nib].as_usize();
            self.descend_first(addr / STAK, addr % STAK);
        }
        true
    }

    #[inline]
    fn backtrack_to_next(&mut self) -> Option<(&[u8], &T)> {
        loop {
            let (parent_encoded, parent_mask, child_nib, parent_vnode) = self.stack.pop()?;
            if self.push_next_child(parent_encoded, parent_mask, child_nib + 1, parent_vnode) {
                return self.current();
            }
        }
    }

    pub fn current(&self) -> Option<(&[u8], &T)> {
        let (_, _, nib, vnode_idx) = self.stack.last()?;
        if *nib == usize::MAX {
            return None;
        }
        let (encoded, _, _, _) = self.stack.last()?;
        let phys_idx = encoded.as_usize() / STAK;
        let node = &self.trie.arena[phys_idx];
        if *nib == TERMINAL_NIB {
            let ki = node.leaf;
            let (off, len) = self.trie.index[ki.as_usize()];
            let key = &self.trie.buf[off..off + len.as_usize()];
            let value = &self.trie.values[ki.as_usize() - 1];
            Some((key, value))
        } else if let Some(key_index) = node.leaf_key_index(*nib, *vnode_idx) {
            let key = self.trie.key_slice(key_index);
            let value = &self.trie.values[key_index.as_usize() - 1];
            Some((key, value))
        } else {
            None
        }
    }

    pub fn current_index(&self) -> Option<usize> {
        let &(_, _, nib, vnode_idx) = self.stack.last()?;
        if nib == usize::MAX {
            return None;
        }
        let (encoded, _, _, _) = self.stack.last()?;
        let phys_idx = encoded.as_usize() / STAK;
        let node = &self.trie.arena[phys_idx];
        if nib == TERMINAL_NIB {
            Some(node.leaf.as_usize())
        } else {
            node.leaf_key_index(nib, vnode_idx).map(|ki| ki.as_usize())
        }
    }

    #[inline]
    fn advance_next(&mut self) -> bool {
        loop {
            let (encoded, mask, nib, vnode_idx) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                if self.push_next_child(encoded, mask, 0, vnode_idx) {
                    return true;
                }
                continue;
            }

            let search_start = if nib == usize::MAX { 0 } else { nib + 1 };
            if self.push_next_child(encoded, mask, search_start, vnode_idx) {
                return true;
            }
        }
    }

    #[inline]
    fn advance_prev(&mut self) -> bool {
        loop {
            let (encoded, mask, nib, vnode_idx) = match self.stack.pop() {
                Some(v) => v,
                None => return false,
            };

            if nib == TERMINAL_NIB {
                continue;
            }

            if nib == 0 || nib == usize::MAX {
                let phys_idx = encoded.as_usize() / STAK;
                if self.trie.arena[phys_idx].is_terminal(vnode_idx) {
                    self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                    return true;
                }
                continue;
            }

            let mask_below = mask & ((1 << nib) - 1);
            if mask_below != 0 {
                let prev_nib = 15 - mask_below.leading_zeros() as usize;
                let phys_idx = encoded.as_usize() / STAK;
                self.stack.push((encoded, mask, prev_nib, vnode_idx));
                if !self.trie.arena[phys_idx].is_leaf(prev_nib, vnode_idx) {
                    let addr = self.trie.arena[phys_idx].children[prev_nib].as_usize();
                    self.descend_last(addr / STAK, addr % STAK);
                }
                return true;
            }

            let phys_idx = encoded.as_usize() / STAK;
            if self.trie.arena[phys_idx].is_terminal(vnode_idx) {
                self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
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
        let mut vnode_idx: usize = 0;
        let max_nib = key.len() * 2;

        loop {
            let node = &self.trie.arena[phys_idx];
            let mask = node.occupancy[vnode_idx];

            if node.is_terminal(vnode_idx) && node.prefix_len[vnode_idx].as_usize() >= max_nib {
                let ki = node.leaf;
                let (off, len) = self.trie.index[ki.as_usize()];
                let node_key = &self.trie.buf[off..off + len.as_usize()];
                if node_key >= key {
                    let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                    self.stack.push((encoded, mask, TERMINAL_NIB, vnode_idx));
                    return self.current();
                }
            }

            if node.prefix_len[vnode_idx].as_usize() >= max_nib {
                let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                if self.push_next_child(encoded, mask, 0, vnode_idx) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let nib = key_nibble_at(key, node.prefix_len[vnode_idx].as_usize()) as usize;
            if !node.is_occupied(nib, vnode_idx) {
                // No child at this nibble — find next higher child, or backtrack
                let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
                if self.push_next_child(encoded, mask, nib + 1, vnode_idx) {
                    return self.current();
                }
                return self.backtrack_to_next();
            }

            let encoded = PTR::from_usize(phys_idx * STAK + vnode_idx);
            self.stack.push((encoded, mask, nib, vnode_idx));
            let slot = node.children[nib];
            if node.is_leaf(nib, vnode_idx) {
                let leaf_key = self.trie.key_slice(slot);
                if leaf_key >= key {
                    return self.current();
                }
                // Leaf key < seek key: advance past it
                return self.next();
            } else {
                let addr = slot.as_usize();
                phys_idx = addr / STAK;
                vnode_idx = addr % STAK;
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

impl TinyTrieMap for NibbleTrie<Vec<u8>, usize, u32, u32, 1> {
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