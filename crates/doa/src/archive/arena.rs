//! Arena — owns the append-only `VecDeque<Block>` plus a side table of
//! order-maintenance tags giving O(1) logical-order compare of block_ids.
//!
//! block_id is a STABLE append index (we only `push_back`, never `push_front`
//! or remove-in-place), so `tags[block_id]` stays aligned. block_id is NOT the
//! block's position in the linked list — that order lives in the tag and in the
//! blocks' own `prev`/`next` fields (block.rs). A split appends a fresh block and
//! splices it between its siblings via `prev`/`next`; its tag is the midpoint of
//! the siblings' tags so `cmp_block` reflects the new list position immediately.
//!
//! Also holds a queue of recent insert hints. Insert is a skeleton; fleshed out
//! with the block layer + remediation (arena_insertion.md).

use std::cmp::Ordering;
use std::collections::VecDeque;

use crate::block::Block;
use crate::index::{SignedBlockIndex, UnsignedIndex};

/// Order-maintenance tag for block linked-list position. Compare two block_ids
/// by tag in O(1); a between-insert takes the midpoint of its siblings' tags.
///
/// Layout: first block `FIRST_TAG`; sequential append/prepend steps by `TAG_STEP`
/// (`1 << 32`), so ~2^31 end-inserts before overflow and ~32 halvings fit between
/// two originally-adjacent tags. Insert-between with adjacent tags (midpoint
/// rounds to the lower tag) needs a relabel of a short run — TODO.
const FIRST_TAG: u64 = 1 << 63;
const TAG_STEP: u64 = 1 << 32;

pub struct Arena<T, U: UnsignedIndex, I: SignedBlockIndex> {
    blocks:   VecDeque<Block<T, I>>,
    /// Order-maintenance tag per block_id. Append-only -> block_id is a stable
    /// index. Tag order == block linked-list order (NOT block_id order).
    tags:     Vec<u64>,
    /// Last ~16 insert hints: `(block_id, addr)` — a Ptr.
    requests: VecDeque<(U, I)>,
}

fn x() {
    let y = 55i32;
}

impl<T, U: UnsignedIndex, I: SignedBlockIndex> Arena<T, U, I> {
    pub fn new() -> Self {
        Self { blocks: VecDeque::new(), tags: Vec::new(), requests: VecDeque::new() }
    }

    /// Append a block and splice it into the linked list between `prev` and
    /// `next` (None = the list end in that direction). Returns the new stable
    /// block_id. Caller guarantees `prev`/`next` are adjacent in the current
    /// list (or an end); this sets the new block's `prev`/`next`, relinks the
    /// neighbors, and assigns a tag.
    pub(crate) fn append_block(
        &mut self,
        mut block: Block<T, I>,
        prev: Option<usize>,
        next: Option<usize>,
    ) -> usize {
        let new_id = self.blocks.len();
        debug_assert_eq!(new_id, self.tags.len());

        // Tag encodes list position. End-inserts step by TAG_STEP; a between-insert
        // takes the overflow-safe midpoint `a + (b-a)/2` (not `(a+b)/2`). Adjacent
        // tags (diff 1) round to `prev` → collision, caught below; relabel TODO.
        let tag = match (prev, next) {
            (None, None)       => FIRST_TAG,                              // first block
            (Some(p), None)    => self.tags[p] + TAG_STEP,               // append at back
            (None, Some(n))    => self.tags[n] - TAG_STEP,               // prepend at front
            (Some(p), Some(n)) => self.tags[p] + (self.tags[n] - self.tags[p]) / 2, // insert between
        };
        debug_assert!(
            !self.tags[..new_id].contains(&tag),
            "tag collision (adjacent siblings): relabel TODO"
        );

        block.prev = prev;
        block.next = next;
        self.blocks.push_back(block);
        self.tags.push(tag);

        if let Some(p) = prev { self.blocks[p].next = Some(new_id); }
        if let Some(n) = next { self.blocks[n].prev = Some(new_id); }

        new_id
    }

    /// Compare two block_ids by linked-list order (tag). O(1).
    #[inline]
    pub fn cmp_block(&self, a: usize, b: usize) -> Ordering {
        self.tags[a].cmp(&self.tags[b])
    }

    /// Compare two Ptrs `(block_id, addr)` by logical order: same block -> addr
    /// (virt is monotonic in sequence position under `addr_shift`); else tag.
    /// This is the key-free descent primitive — search a tree where child Ptrs
    /// are the keys without ever seeing a key value.
    #[inline]
    pub fn cmp_ptr(&self, a: (U, I), b: (U, I)) -> Ordering {
        if a.0 == b.0 {
            a.1.cmp(&b.1)
        } else {
            self.cmp_block(a.0.as_usize(), b.0.as_usize())
        }
    }

    pub fn insert_before(&mut self, _at: (U, I), _val: T) {
        todo!("GP4: block-level insert_before with auto-split/grow + RemapSet")
    }
    pub fn insert_after(&mut self, _at: (U, I), _val: T) {
        todo!("GP4: block-level insert_after with auto-split/grow + RemapSet")
    }
}

impl<T, U: UnsignedIndex, I: SignedBlockIndex> Default for Arena<T, U, I> {
    fn default() -> Self {
        Self::new()
    }
}