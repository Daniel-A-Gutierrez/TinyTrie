//! Variable-length key B+ tree using packed key storage.
//!
//! This module implements a B+ tree optimized for variable-length byte keys.
//! Keys are stored in `PackedKeySlots<N>` — a dense layout where each key's
//! full bytes are stored contiguously with no padding. The sequential scan
//! walks through packed bytes with a running offset.

use std::num::{NonZero, ZeroablePrimitive};

use smallvec::SmallVec;

pub use crate::packed_keys::LengthType;
use crate::packed_keys::PackedKeySlots;

// ---------------------------------------------------------------------------
// TrieIndex
// ---------------------------------------------------------------------------

/// Index type for arena-based node pointers.
pub trait TrieIndex:
    Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug + 'static + ZeroablePrimitive
{
    fn as_usize(self) -> usize;
    fn max_value() -> usize;
    fn from_usize(n: usize) -> Self;
}

macro_rules! impl_trie_index {
    ($($ty:ty),* $(,)?) => {
        $(
            impl TrieIndex for $ty {
                #[inline] fn as_usize(self) -> usize { self as usize }
                #[inline] fn max_value() -> usize { <$ty>::MAX as usize }
                #[inline] fn from_usize(n: usize) -> Self { n as $ty }
            }
        )*
    };
}

impl_trie_index!(u8, u16, u32, u64);

// ---------------------------------------------------------------------------
// VarKey trait
// ---------------------------------------------------------------------------

/// Trait for variable-length key types that can be stored in a VarCTree.
pub trait VarKey: Ord + Clone + 'static {
    type Needle: ?Sized + AsRef<[u8]>;
    fn as_needle(&self) -> &Self::Needle;
    fn into_bytes(self) -> Vec<u8>;
}

impl VarKey for Vec<u8> {
    type Needle = [u8];
    fn as_needle(&self) -> &[u8] { self }
    fn into_bytes(self) -> Vec<u8> { self }
}

impl VarKey for Box<[u8]> {
    type Needle = [u8];
    fn as_needle(&self) -> &[u8] { self }
    fn into_bytes(self) -> Vec<u8> { Vec::from(self) }
}

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

struct KeyNode<PTR, L, const N: usize, const NP1: usize>
where
    PTR: TrieIndex,
    L: LengthType,
    [(); N]:,
    [(); NP1]:,
{
    keys: PackedKeySlots<L, N>,
    ptrs: [Option<NonZero<PTR>>; NP1],
}

struct LeafNode<V, PTR, L, const N: usize>
where
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
{
    keys: PackedKeySlots<L, N>,
    values: Vec<V>,
    prev: Option<NonZero<PTR>>,
    next: Option<NonZero<PTR>>,
}

// ---------------------------------------------------------------------------
// KeyNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<PTR, L, const N: usize, const NP1: usize> KeyNode<PTR, L, N, NP1>
where
    PTR: TrieIndex,
    L: LengthType,
    [(); N]:,
    [(); NP1]:,
{
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    fn new() -> Self {
        Self {
            keys: PackedKeySlots::new(),
            ptrs: [None; NP1],
        }
    }

    fn from_parent(from: usize, to: usize, parent: &Self) -> Self {
        let mut node = Self::new();
        for i in from..to {
            let key = parent.keys.get_key(i);
            node.keys.insert_at(i - from, &key);
        }
        for i in from..=to {
            node.ptrs[i - from] = parent.ptrs[i];
        }
        node
    }

    #[inline]
    fn get_ptr(&self, i: usize) -> Option<usize> {
        debug_assert!(i <= self.keys.len());
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    unsafe fn get_ptr_unchecked(&self, i: usize) -> Option<usize> {
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn set_ptr(&mut self, i: usize, idx: usize) {
        self.ptrs[i] = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn clear_ptr(&mut self, i: usize) {
        self.ptrs[i] = None;
    }

    #[inline]
    fn find_position(&self, needle: &[u8]) -> usize {
        self.keys.find_position(needle)
    }

    #[inline]
    fn find_child(&self, needle: &[u8]) -> usize {
        self.keys.find_upper_bound(needle)
    }

    #[inline]
    fn adjacent_sibling_ptrs(&self, child_pos: usize) -> (Option<usize>, Option<usize>) {
        let left = if child_pos > 0 { self.get_ptr(child_pos - 1) } else { None };
        let right = if child_pos < self.keys.len() { self.get_ptr(child_pos + 1) } else { None };
        (left, right)
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    #[inline]
    fn would_merge(&self) -> bool {
        self.keys.len() == N / 2
    }

    fn insert_key_at(&mut self, pos: usize, key: &[u8]) {
        debug_assert!(!self.would_split());
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, key);
    }

    fn insert_leaf(&mut self, needle: &[u8], key: &[u8]) -> usize {
        let pos = self.find_position(needle);
        self.insert_key_at(pos, key);
        pos
    }

    fn remove(&mut self, pos: usize) -> Vec<u8> {
        let l = self.keys.len();
        let k = self.keys.remove_at(pos);
        if pos + 1 < l {
            for i in pos + 1..l {
                self.ptrs[i] = self.ptrs[i + 1];
            }
        }
        k
    }

    #[inline]
    fn truncate(&mut self, newlen: u8) {
        self.keys.truncate(newlen);
    }
}

// ---------------------------------------------------------------------------
// LeafNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<V, PTR, L, const N: usize> LeafNode<V, PTR, L, N>
where
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
{
    fn new() -> Self {
        Self {
            keys: PackedKeySlots::new(),
            values: Vec::new(),
            prev: None,
            next: None,
        }
    }

    #[inline]
    fn get_prev(&self) -> Option<usize> {
        self.prev.map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn get_next(&self) -> Option<usize> {
        self.next.map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn set_prev(&mut self, idx: usize) {
        self.prev = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn set_next(&mut self, idx: usize) {
        self.next = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn clear_prev(&mut self) { self.prev = None; }

    #[inline]
    fn clear_next(&mut self) { self.next = None; }

    #[inline]
    fn find_position(&self, needle: &[u8]) -> usize {
        self.keys.find_position(needle)
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    fn insert(&mut self, pos: usize, key: &[u8], value: V) {
        self.keys.insert_at(pos, key);
        self.values.insert(pos, value);
    }

    fn remove(&mut self, pos: usize) -> (Vec<u8>, V) {
        let k = self.keys.remove_at(pos);
        let v = self.values.remove(pos);
        (k, v)
    }

    fn truncate(&mut self, newlen: u8) {
        self.keys.truncate(newlen);
        self.values.truncate(newlen as usize);
    }
}

// ---------------------------------------------------------------------------
// Cursor navigation macro (reduces duplication between Cursor and CursorMut)
// ---------------------------------------------------------------------------

/// Generates `next()` and `prev()` methods for cursor types.
/// Pass `&` for Cursor (returns `&V`) or `& mut` for CursorMut (returns `&mut V`).
macro_rules! impl_cursor_nav {
    (& $($mut:tt)?) => {
        pub fn next(&mut self) -> Option<& $($mut)? V> {
            let leaf = &self.tree.leaves[self.leaf_idx];
            if self.position + 1 < leaf.keys.len() {
                self.packed_off += leaf.keys.key_len(self.position);
                self.position += 1;
            } else {
                let next_leaf = leaf.get_next()?;
                self.leaf_idx = next_leaf;
                self.position = 0;
                self.packed_off = 0;
            }
            Some(& $($mut)? self.tree.leaves[self.leaf_idx].values[self.position])
        }

        pub fn prev(&mut self) -> Option<& $($mut)? V> {
            if self.position > 0 {
                self.packed_off -= self.tree.leaves[self.leaf_idx].keys.key_len(self.position - 1);
                self.position -= 1;
            } else {
                let prev_leaf = self.tree.leaves[self.leaf_idx].get_prev()?;
                self.leaf_idx = prev_leaf;
                let last_pos = self.tree.leaves[self.leaf_idx].keys.len() - 1;
                self.position = last_pos;
                self.packed_off = self.tree.leaves[self.leaf_idx].keys.packed_offset_up_to(last_pos);
            }
            Some(& $($mut)? self.tree.leaves[self.leaf_idx].values[self.position])
        }
    };
}

// ---------------------------------------------------------------------------
// two_mut
// ---------------------------------------------------------------------------

#[inline]
fn two_mut<T>(slice: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    debug_assert_ne!(a, b, "two_mut: indices must differ");
    if a < b {
        let (left, right) = slice.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = slice.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}

// ---------------------------------------------------------------------------
// VarCTree
// ---------------------------------------------------------------------------

/// B+ tree for variable-length byte keys using packed key storage.
pub struct VarCTree<K, V, PTR, L, const N: usize, const NP1: usize>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    inodes: Vec<KeyNode<PTR, L, N, NP1>>,
    leaves: Vec<LeafNode<V, PTR, L, N>>,
    len: usize,
    n_leaves: usize,
    height: usize,
    root_inode: usize,
    _phantom: std::marker::PhantomData<K>,
}

#[allow(dead_code)]
impl<K, V, PTR, L, const N: usize, const NP1: usize>
    VarCTree<K, V, PTR, L, N, NP1>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    #[inline]
    fn rebalance_target(s: usize) -> usize {
        (N + s) / 2
    }

    pub fn new() -> Self {
        let () = Self::ASSERT_NP1;
        let () = Self::ASSERT_N_FITS;
        let root = LeafNode::<V, PTR, L, N>::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
            len: 0,
            n_leaves: 1,
            height: 0,
            root_inode: 0,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    pub fn compact(&mut self) {
        self.relocate(false);
        self.inodes.shrink_to_fit();
    }

    pub fn optimize(&mut self) {
        self.relocate(false);
    }

    fn relocate(&mut self, gapful: bool) -> Vec<usize> {
        let old_len = self.leaves.len();
        let mut order: Vec<usize> = Vec::with_capacity(self.n_leaves);
        let mut idx = self.first_leaf();
        order.push(idx);
        while let Some(nx) = self.leaves[idx].get_next() {
            order.push(nx);
            idx = nx;
        }
        let live = order.len();
        debug_assert_eq!(live, self.n_leaves);

        let slot_of = |rank: usize| if gapful { 2 * rank } else { rank };
        let new_slots = slot_of(live);
        let mut new_pos = vec![usize::MAX; old_len];
        for rank in 0..live {
            new_pos[order[rank]] = slot_of(rank);
        }

        let mut old = std::mem::take(&mut self.leaves);
        let mut buf: Vec<LeafNode<V, PTR, L, N>> = Vec::with_capacity(new_slots);
        for i in 0..new_slots {
            let is_live_slot = !gapful || i % 2 == 0;
            if is_live_slot {
                let rank = if gapful { i / 2 } else { i };
                let old_idx = order[rank];
                let leaf = std::mem::replace(&mut old[old_idx], LeafNode::new());
                buf.push(leaf);
            } else {
                buf.push(LeafNode::new());
            }
        }
        drop(old);

        for rank in 0..live {
            let i = slot_of(rank);
            if rank > 0 { buf[i].set_prev(slot_of(rank - 1)); }
            else { buf[i].clear_prev(); }
            if rank + 1 < live { buf[i].set_next(slot_of(rank + 1)); }
            else { buf[i].clear_next(); }
        }

        if self.height >= 1 {
            let mut level: Vec<usize> = vec![self.root_inode];
            for _ in 0..self.height - 1 {
                let mut next = Vec::new();
                for &ni in &level {
                    let node = &self.inodes[ni];
                    for ci in 0..=node.keys.len() {
                        if let Some(c) = node.get_ptr(ci) { next.push(c); }
                    }
                }
                level = next;
            }
            for &ni in &level {
                let node = &mut self.inodes[ni];
                let klen = node.keys.len();
                for ci in 0..=klen {
                    if let Some(c) = node.get_ptr(ci) {
                        node.set_ptr(ci, new_pos[c]);
                    }
                }
            }
        }

        self.leaves = buf;
        self.n_leaves = live;
        new_pos
    }

    fn spread(&mut self) -> Vec<usize> { self.relocate(true) }

    fn claim_slot(&self, after: usize) -> usize {
        let n = self.leaves.len();
        let mut i = after + 1;
        while i < n {
            if self.leaves[i].keys.len() == 0 { return i; }
            i += 1;
        }
        for i in 0..after {
            if self.leaves[i].keys.len() == 0 { return i; }
        }
        unreachable!("claim_slot: no free gap")
    }

    /// Maximum depth for recursive rebalance cascading.
    /// When a node is full and its immediate sibling is also full,
    /// we recursively try to make room in the sibling by rebalancing
    /// it with its own sibling in the same direction, up to this depth.
    const REBALANCE_DEPTH: usize = 3;

    fn walk_to_leaf(&mut self, needle: &[u8]) -> (usize, SmallVec<[(usize, usize); 8]>) {
        if self.height == 0 { return (0, SmallVec::new()); }
        let mut path = SmallVec::new();
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle);
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            let mut child = child;
            if self.inodes[child_idx].would_split()
                && self.try_rebalance_inode(node_idx, child, Self::REBALANCE_DEPTH)
            {
                child = self.inodes[node_idx].find_child(needle);
            }
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            path.push((node_idx, child));
            node_idx = child_idx;
        }
        let child = self.inodes[node_idx].find_child(needle);
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        let mut child = child;
        if self.leaves[leaf_idx].would_split()
            && self.try_rebalance_leaf(node_idx, child, Self::REBALANCE_DEPTH)
        {
            child = self.inodes[node_idx].find_child(needle);
        }
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        path.push((node_idx, child));
        (leaf_idx, path)
    }

    #[inline]
    fn find_leaf(&self, needle: &[u8]) -> usize {
        if self.height == 0 { return 0; }
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle);
            node_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        }
        let bottom = &self.inodes[node_idx];
        let child = bottom.find_child(needle);
        bottom.get_ptr(child).unwrap()
    }

    #[inline]
    fn leaf_has_room_for_two(&self, idx: usize) -> bool {
        self.leaves[idx].keys.len() + 2 <= N
    }

    #[inline]
    fn inode_has_room_for_two(&self, idx: usize) -> bool {
        self.inodes[idx].keys.len() + 2 <= N
    }

    /// Try to rebalance a full leaf by redistributing keys to a sibling.
    /// If the immediate sibling is also full, recursively try to make room
    /// in it by rebalancing *it* with its own sibling in the same direction,
    /// up to `depth` levels deep. Left keeps checking left; right keeps
    /// checking right — never rebalancing back toward the source node.
    fn try_rebalance_leaf(&mut self, parent_idx: usize, child_pos: usize, depth: usize) -> bool {
        let leaf_idx = self.inodes[parent_idx].get_ptr(child_pos).unwrap();
        let parent = &self.inodes[parent_idx];
        let left_pos = if child_pos > 0 { Some(child_pos - 1) } else { None };
        let right_pos = if child_pos < parent.keys.len() { Some(child_pos + 1) } else { None };
        let left_idx = left_pos.and_then(|p| parent.get_ptr(p));
        let right_idx = right_pos.and_then(|p| parent.get_ptr(p));

        // Decide which direction to try first (prefer the less-full sibling)
        let try_right_first = match (left_idx, right_idx) {
            (Some(l), Some(r)) => self.leaves[r].keys.len() <= self.leaves[l].keys.len(),
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => return false,
        };

        // Try directions in priority order
        let directions: [(Option<usize>, Option<usize>, bool); 2] = if try_right_first {
            [(right_idx, right_pos, true), (left_idx, left_pos, false)]
        } else {
            [(left_idx, left_pos, false), (right_idx, right_pos, true)]
        };

        for (sib_idx, sib_pos, go_right) in directions {
            let Some(sib_idx) = sib_idx else { continue };
            let Some(sib_pos) = sib_pos else { continue };

            if self.leaf_has_room_for_two(sib_idx) {
                self.redistribute_leaf_dir(parent_idx, child_pos, leaf_idx, sib_idx, go_right);
                return true;
            }
            // Sibling is full — cascade in the same direction
            if depth > 0 && self.try_rebalance_leaf(parent_idx, sib_pos, depth - 1) {
                // Sibling may now have room
                if self.leaf_has_room_for_two(sib_idx) {
                    self.redistribute_leaf_dir(parent_idx, child_pos, leaf_idx, sib_idx, go_right);
                    return true;
                }
            }
        }

        false
    }

    /// Dispatch to redistribute_leaf_left or redistribute_leaf_right.
    fn redistribute_leaf_dir(
        &mut self, parent_idx: usize, child_pos: usize,
        leaf_idx: usize, sib_idx: usize, go_right: bool,
    ) {
        if go_right {
            self.redistribute_leaf_right(parent_idx, child_pos, leaf_idx, sib_idx);
        } else {
            self.redistribute_leaf_left(parent_idx, child_pos, leaf_idx, sib_idx);
        }
    }

    /// Try to rebalance a full inode by redistributing keys to a sibling.
    /// Same recursive cascading logic as try_rebalance_leaf.
    fn try_rebalance_inode(&mut self, gparent_idx: usize, child_pos: usize, depth: usize) -> bool {
        let l_idx = self.inodes[gparent_idx].get_ptr(child_pos).unwrap();
        let parent = &self.inodes[gparent_idx];
        let left_pos = if child_pos > 0 { Some(child_pos - 1) } else { None };
        let right_pos = if child_pos < parent.keys.len() { Some(child_pos + 1) } else { None };
        let left_idx = left_pos.and_then(|p| parent.get_ptr(p));
        let right_idx = right_pos.and_then(|p| parent.get_ptr(p));

        let try_right_first = match (left_idx, right_idx) {
            (Some(l), Some(r)) => self.inodes[r].keys.len() <= self.inodes[l].keys.len(),
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => return false,
        };

        let directions: [(Option<usize>, Option<usize>, bool); 2] = if try_right_first {
            [(right_idx, right_pos, true), (left_idx, left_pos, false)]
        } else {
            [(left_idx, left_pos, false), (right_idx, right_pos, true)]
        };

        for (sib_idx, sib_pos, go_right) in directions {
            let Some(sib_idx) = sib_idx else { continue };
            let Some(sib_pos) = sib_pos else { continue };

            if self.inode_has_room_for_two(sib_idx) {
                self.redistribute_inode_dir(gparent_idx, child_pos, l_idx, sib_idx, go_right);
                return true;
            }
            if depth > 0 && self.try_rebalance_inode(gparent_idx, sib_pos, depth - 1) {
                if self.inode_has_room_for_two(sib_idx) {
                    self.redistribute_inode_dir(gparent_idx, child_pos, l_idx, sib_idx, go_right);
                    return true;
                }
            }
        }

        false
    }

    /// Dispatch to redistribute_inode_left or redistribute_inode_right.
    fn redistribute_inode_dir(
        &mut self, gparent_idx: usize, child_pos: usize,
        l_idx: usize, sib_idx: usize, go_right: bool,
    ) {
        if go_right {
            self.redistribute_inode_right(gparent_idx, child_pos, l_idx, sib_idx);
        } else {
            self.redistribute_inode_left(gparent_idx, child_pos, l_idx, sib_idx);
        }
    }

    /// Move keys from the full node to its right sibling.
    /// Separator in parent is at `child_pos`.
    /// Mirror of `redistribute_leaf_left` (which moves to the left sibling,
    /// separator at `child_pos - 1`).
    fn redistribute_leaf_right(
        &mut self, parent_idx: usize, child_pos: usize,
        leaf_idx: usize, sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s);
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_into_front(l_target, &mut sib.keys);
            let drain: Vec<V> = leaf.values.drain(l_target..).collect();
            sib.values.splice(0..0, drain);
        }
        // Safe: leaves and inodes are disjoint fields
        let new_sep = self.leaves[sib_idx].keys.key_slice(0);
        self.inodes[parent_idx].keys.update_at(child_pos, new_sep);
    }

    /// Move keys from the full node to its left sibling.
    /// Separator in parent is at `child_pos - 1`.
    /// Mirror of `redistribute_leaf_right` (which moves to the right sibling,
    /// separator at `child_pos`).
    fn redistribute_leaf_left(
        &mut self, parent_idx: usize, child_pos: usize,
        leaf_idx: usize, sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_front_into(m, &mut sib.keys);
            let drain: Vec<V> = leaf.values.drain(..m).collect();
            sib.values.extend(drain);
        }
        // Safe: leaves and inodes are disjoint fields
        let new_sep = self.leaves[leaf_idx].keys.key_slice(0);
        self.inodes[parent_idx].keys.update_at(child_pos - 1, new_sep);
    }

    fn redistribute_inode_right(
        &mut self, gparent_idx: usize, child_pos: usize,
        l_idx: usize, r_idx: usize,
    ) {
        let (s, sep0) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[r_idx].keys.len(), g.keys.get_key(child_pos))
        };
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;

        let new_sep = {
            let (l, r) = two_mut(&mut self.inodes, l_idx, r_idx);
            r.ptrs.copy_within(0..=s, m);
            for i in 0..m {
                r.ptrs[i] = l.ptrs[l_target + 1 + i].take();
            }
            if m > 1 {
                l.keys.drain_into_front(l_target + 1, &mut r.keys);
            }
            r.keys.insert_at(m - 1, &sep0);
            l.keys.remove_at(l_target)
        };
        self.inodes[gparent_idx].keys.update_at(child_pos, &new_sep);
    }

    fn redistribute_inode_left(
        &mut self, gparent_idx: usize, child_pos: usize,
        l_idx: usize, sib_idx: usize,
    ) {
        let (s, sep0) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[sib_idx].keys.len(), g.keys.get_key(child_pos - 1))
        };
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;

        let new_sep = {
            let (l, sib) = two_mut(&mut self.inodes, l_idx, sib_idx);
            for i in 0..m {
                sib.ptrs[(s + 1) + i] = l.ptrs[i].take();
            }
            sib.keys.push(&sep0);
            if m > 1 {
                l.keys.drain_front_into(m - 1, &mut sib.keys);
            }
            l.keys.remove_at(0)
        };
        {
            let l = &mut self.inodes[l_idx];
            l.ptrs.copy_within(m..=N, 0);
            for i in (N - m + 1)..=N {
                l.ptrs[i] = None;
            }
        }
        self.inodes[gparent_idx].keys.update_at(child_pos - 1, &new_sep);
    }

    #[inline]
    fn locate_with_offset(&self, needle: &[u8]) -> (usize, usize, usize) {
        let leaf_idx = self.find_leaf(needle);
        let (pos, off) = self.leaves[leaf_idx].keys.find_position_with_offset(needle);
        (leaf_idx, pos, off)
    }

    #[inline]
    fn locate(&self, needle: &[u8]) -> (usize, usize) {
        let (leaf_idx, pos, _) = self.locate_with_offset(needle);
        (leaf_idx, pos)
    }

    #[inline]
    pub fn get(&self, key: &K::Needle) -> Option<&V> {
        if self.leaves.is_empty() { return None; }
        let needle = key.as_ref();
        let (leaf_idx, pos, off) = self.locate_with_offset(needle);
        let leaf = &self.leaves[leaf_idx];
        if pos < leaf.keys.len() {
            if leaf.keys.eq_key_with_offset(pos, off, needle) {
                return Some(&leaf.values[pos]);
            }
        }
        None
    }

    #[inline]
    pub fn get_mut(&mut self, key: &K::Needle) -> Option<&mut V> {
        if self.leaves.is_empty() { return None; }
        let needle = key.as_ref();
        let (leaf_idx, pos, off) = self.locate_with_offset(needle);
        if pos < self.leaves[leaf_idx].keys.len() {
            if self.leaves[leaf_idx].keys.eq_key_with_offset(pos, off, needle) {
                return Some(&mut self.leaves[leaf_idx].values[pos]);
            }
        }
        None
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<(), (K, V)> {
        let _ = Self::ASSERT_N_FITS;
        let needle = key.as_needle();
        let needle_bytes = needle.as_ref();
        let (child_idx, path) = self.walk_to_leaf(needle_bytes);
        let (pos, off) = self.leaves[child_idx].keys.find_position_with_offset(needle_bytes);

        // Key already exists?
        if pos < self.leaves[child_idx].keys.len()
            && self.leaves[child_idx].keys.eq_key_with_offset(pos, off, needle_bytes)
        {
            return Err((key, value));
        }

        if needle_bytes.len() > <L as LengthType>::max() {
            return Err((key, value));
        }

        let key_bytes = key.into_bytes();

        if self.leaves[child_idx].keys.len() >= N {
            let mid = N / 2;
            let (parent_idx, new_leaf_idx) = self.split_leaf(child_idx, path);
            if pos <= mid {
                self.leaves[parent_idx].insert(pos, &key_bytes, value);
            } else {
                self.leaves[new_leaf_idx].insert(pos - mid, &key_bytes, value);
            }
        } else {
            self.leaves[child_idx].insert(pos, &key_bytes, value);
        }

        self.len += 1;
        Ok(())
    }

    fn split_leaf(&mut self, child_idx: usize, mut path: SmallVec<[(usize, usize); 8]>) -> (usize, usize) {
        let mid = N / 2;
        let mid_key = self.leaves[child_idx].keys.get_key(mid);

        let child_idx = if self.n_leaves == self.leaves.len() {
            let map = self.spread();
            map[child_idx]
        } else {
            child_idx
        };

        let old_next = self.leaves[child_idx].get_next();
        let drain_bytes = self.leaves[child_idx].keys.packed_len()
            - self.leaves[child_idx].keys.packed_offset_up_to(mid);
        let mut new_leaf = LeafNode::<V, PTR, L, N>::new();
        new_leaf.keys.reserve(drain_bytes * 2);
        self.leaves[child_idx].keys.drain_into(mid, &mut new_leaf.keys);
        let drained_values = self.leaves[child_idx].values.split_off(mid);
        new_leaf.values = drained_values;

        let new_leaf_idx = self.claim_slot(child_idx);
        new_leaf.set_prev(child_idx);
        if let Some(ni) = old_next { new_leaf.set_next(ni); }
        self.leaves[child_idx].set_next(new_leaf_idx);
        self.leaves[new_leaf_idx] = new_leaf;
        if let Some(next_idx) = old_next {
            self.leaves[next_idx].set_prev(new_leaf_idx);
        }

        self.n_leaves += 1;
        self.insert_separator(&mid_key, new_leaf_idx, &mut path);
        (child_idx, new_leaf_idx)
    }

    fn insert_separator(&mut self, stored: &[u8], new_child_idx: usize, path: &mut SmallVec<[(usize, usize); 8]>) {
        if path.is_empty() {
            let old_root_idx = self.root_inode;
            let mut root = KeyNode::<PTR, L, N, NP1>::new();
            root.keys.insert_at(0, &stored);
            root.set_ptr(0, old_root_idx);
            root.set_ptr(1, new_child_idx);
            let root_idx = self.inodes.len();
            self.inodes.push(root);
            self.root_inode = root_idx;
            self.height += 1;
            return;
        }

        let (parent_idx, _) = path.pop().unwrap();

        if !self.inodes[parent_idx].would_split() {
            let pos = self.find_position_for_stored(&stored, &self.inodes[parent_idx].keys);
            self.inodes[parent_idx].insert_key_at(pos, &stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        } else {
            self.split_inode(parent_idx, stored, new_child_idx, path);
        }
    }

    fn split_inode(
        &mut self,
        parent_idx: usize,
        new_stored: &[u8],
        new_child_idx: usize,
        path: &mut SmallVec<[(usize, usize); 8]>,
    ) {
        let mid = N / 2;
        let mid_stored = self.inodes[parent_idx].keys.get_key(mid);
        let old_len = self.inodes[parent_idx].keys.len();

        let mut new_inode = KeyNode::<PTR, L, N, NP1>::new();
        // Move keys [mid+1..old_len) to new inode.
        if mid + 1 < old_len {
            let drain_bytes = self.inodes[parent_idx].keys.packed_len()
                - self.inodes[parent_idx].keys.packed_offset_up_to(mid + 1);
            new_inode.keys.reserve(drain_bytes * 2);
            self.inodes[parent_idx].keys.drain_into(mid + 1, &mut new_inode.keys);
        }
        // Move ptrs [mid+1..=old_len] to new inode.
        for i in 0..=old_len - (mid + 1) {
            new_inode.ptrs[i] = self.inodes[parent_idx].ptrs[mid + 1 + i];
        }
        // Remove the mid separator key.
        self.inodes[parent_idx].keys.remove_at(mid);
        // Clear moved ptrs.
        for i in (mid + 1)..=old_len {
            self.inodes[parent_idx].ptrs[i] = None;
        }

        // Insert the new key/child into the appropriate inode.
        // Compare against the separator key that was removed — if new_stored
        // is >= the separator, it goes into the right (new) inode.
        let goes_right = new_stored.cmp(&mid_stored) != std::cmp::Ordering::Less;
        if goes_right {
            let pos = self.find_position_for_stored(&new_stored, &new_inode.keys);
            new_inode.insert_key_at(pos, &new_stored);
            new_inode.set_ptr(pos + 1, new_child_idx);
        } else {
            let pos = self.find_position_for_stored(&new_stored, &self.inodes[parent_idx].keys);
            self.inodes[parent_idx].insert_key_at(pos, &new_stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        }

        let new_inode_idx = self.inodes.len();
        self.inodes.push(new_inode);
        self.insert_separator(&mid_stored, new_inode_idx, path);
    }

    fn find_position_for_stored(&self, stored: &[u8], keys: &PackedKeySlots<L, N>) -> usize {
        for i in 0..keys.len() {
            let key = keys.key_slice(i);
            if stored.cmp(key) != std::cmp::Ordering::Greater {
                return i;
            }
        }
        keys.len()
    }

    fn descend_to_leaf(&self, rightmost: bool) -> usize {
        if self.height == 0 { return 0; }
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let ci = if rightmost { self.inodes[node_idx].keys.len() } else { 0 };
            node_idx = self.inodes[node_idx].get_ptr(ci).unwrap();
        }
        let ci = if rightmost { self.inodes[node_idx].keys.len() } else { 0 };
        self.inodes[node_idx].get_ptr(ci).unwrap()
    }

    fn first_leaf(&self) -> usize { self.descend_to_leaf(false) }
    fn last_leaf(&self) -> usize { self.descend_to_leaf(true) }

    pub fn get_cursor(&self) -> Cursor<'_, K, V, PTR, L, N, NP1> {
        let leaf_idx = self.first_leaf();
        Cursor { tree: self, leaf_idx, position: 0, packed_off: 0 }
    }

    pub fn get_cursor_mut(&mut self) -> CursorMut<'_, K, V, PTR, L, N, NP1> {
        let leaf_idx = self.first_leaf();
        CursorMut { tree: self, leaf_idx, position: 0, packed_off: 0 }
    }

    pub fn cursor_at(&self, key: &K::Needle) -> Cursor<'_, K, V, PTR, L, N, NP1> {
        let needle = key.as_ref();
        let (leaf_idx, pos, packed_off) = self.locate_with_offset(needle);
        Cursor { tree: self, leaf_idx, position: pos, packed_off }
    }
}

// ---------------------------------------------------------------------------
// Cursor impl
// ---------------------------------------------------------------------------

pub struct Cursor<'a, K, V, PTR, L, const N: usize, const NP1: usize>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    tree: &'a VarCTree<K, V, PTR, L, N, NP1>,
    leaf_idx: usize,
    position: usize,
    /// Cached byte offset into the leaf's packed key buffer.
    packed_off: usize,
}

pub struct CursorMut<'a, K, V, PTR, L, const N: usize, const NP1: usize>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    tree: &'a mut VarCTree<K, V, PTR, L, N, NP1>,
    leaf_idx: usize,
    position: usize,
    packed_off: usize,
}

#[allow(dead_code)]
impl<'a, K, V, PTR, L, const N: usize, const NP1: usize>
    Cursor<'a, K, V, PTR, L, N, NP1>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    pub fn current(&self) -> Option<(&[u8], &V)> {
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position < leaf.keys.len() {
            let (key, _) = leaf.keys.key_slice_with_offset(self.position, self.packed_off);
            Some((key, &leaf.values[self.position]))
        } else {
            None
        }
    }

    impl_cursor_nav!(&);
}

#[allow(dead_code)]
impl<'a, K, V, PTR, L, const N: usize, const NP1: usize>
    CursorMut<'a, K, V, PTR, L, N, NP1>
where
    K: VarKey,
    PTR: TrieIndex,
    L: LengthType,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    pub fn current(&mut self) -> Option<(&[u8], &mut V)> {
        let pos = self.position;
        let off = self.packed_off;
        let leaf = &mut self.tree.leaves[self.leaf_idx];
        if pos < leaf.keys.len() {
            let (key, _) = leaf.keys.key_slice_with_offset(pos, off);
            let value = leaf.values.get_mut(pos)?;
            Some((key, value))
        } else {
            None
        }
    }

    impl_cursor_nav!(& mut);
}

// ---------------------------------------------------------------------------
// Instantiation alias
// ---------------------------------------------------------------------------

/// Variable-length-key B+ tree with packed key storage.
pub type VarCTreeMap<K, V, PTR, L, const N: usize, const NP1: usize> =
    VarCTree<K, V, PTR, L, N, NP1>;

#[cfg(test)]
#[path = "tests/var_btree.rs"]
mod tests;