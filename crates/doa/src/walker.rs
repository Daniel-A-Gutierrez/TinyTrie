use std::marker::PhantomData;

use crate::block::OpenSlot;
use crate::block_cursor::{BlockCursor, Cursor, CursorMut};
use crate::node::{Node, SplittableNode};
use crate::store::NoneSlide;
use crate::tree_block::TreeBlockMut;
use crate::Ordering;

///use as meta on treeblock for fixed height trees (like b+ trees or S trees)
#[derive(Default, Clone, Copy)]
pub struct Height(pub u64);

//node-agnostic constructors: build a cursor from a block borrow + meta. they know
//nothing of the concrete node type, so they live here on `Probe`, generic over `B`/`M`.
impl<'block, 'walker, B, M> Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker B>, M>
where
    'block: 'walker,
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub fn new(meta: M, block: &'walker B) -> Self {
        Self {
            cursor: BlockCursor::new_at(block, block.root()),
            depth: 0,
            meta,
            _l: PhantomData,
        }
    }
}

impl<'block, 'walker, B, M> Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker mut B>, M>
where
    'block: 'walker,
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub fn new_mut(meta: M, block: &'walker mut B) -> Self {
        let root = block.root();
        Self {
            cursor: BlockCursor::new_at(block, root),
            depth: 0,
            meta,
            _l: PhantomData,
        }
    }
}

pub struct Ancestor { phys : usize, child_idx : usize } 
impl Ancestor {
    fn new(phys:  usize, child_idx: usize) -> Self { Self { phys , child_idx }  }
}

pub struct Walker<'block, B, C, M>
where
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub(crate) cursor: C,
    pub(crate) stack: Vec<Ancestor>,
    pub(crate) meta : M,
    _l: PhantomData<&'block B>,
}


impl<'block, 'walker, B, M> Walker<'block, B, BlockCursor<'block, 'walker, B, &'walker mut B>, M>
where
    'block: 'walker,
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub fn new_mut(meta: M, block: &'walker mut B) -> Self {
        let root = block.root();
        Self {
            cursor: BlockCursor::new_at(block, root),
            meta,
            stack : vec![],
            _l: PhantomData,
        }
    }

    //current node is the parent of some of the slid nodes: rewrite each (old_v,new_v) child
    //pointer it still holds.
    fn fixup_stack(&mut self, ns : NoneSlide) {
        todo!();
    }
}

impl<'block, 'walker, B, M> Walker<'block, B, BlockCursor<'block, 'walker, B, &'walker B>, M>
where
    'block: 'walker,
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub fn new(meta: M, block: &'walker B) -> Self {
        Self {
            cursor: BlockCursor::new_at(block, block.root()),
            meta,
            stack : vec![],
            _l: PhantomData,
        }
    }
}

    

//stackless, no alloc.
pub struct Probe<'block, B, C, M>
where
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub(crate) cursor: C,
    pub(crate) depth: u64,
    //tree specific data, cloned on probe creation
    pub meta : M,
    pub(crate) _l: PhantomData<(&'block (), *const B)>,
}

///shared (read) walker. returned `&B::T` are tied to the `&mut self` call borrow
///(no streaming iterator). requires `C: Cursor`. navigation (`descend`/`ascend`/
///`walk_to`/…) is generic over `C` — it touches only `Cursor` methods, and
///`CursorMut: Cursor`, so one impl serves both the shared and mut `Probe`;
///`TreeWalkerMut: TreeWalker` inherits it. construction (`new`/`new_mut`) is
///cursor-specific, so it is inherent on each `Probe` variant, not here.
pub trait TreeWalker<'block, 'walker, O, B, C>
where
    O: Ordering,
    B: TreeBlockMut<'block>,
    B::T: Node,
    'block: 'walker,
    C: Cursor<'walker, B::T, B::P>,
{
    ///returns current after moving to next in ordering
    fn go_next(&mut self) -> Option<&B::T>;
    ///returns current after moving to prev in ordering
    fn go_prev(&mut self) -> Option<&B::T>;
    ///returns current after moving to child at idx
    fn descend(&mut self, idx: usize) -> Option<&B::T>;
    fn descend_right(&mut self, times: usize) -> Option<(&B::T, usize)>;
    fn descend_left(&mut self, times: usize) -> Option<(&B::T, usize)>;
    fn ascend(&mut self) -> Option<&B::T>;
    fn depth(&self) -> usize;
    fn position(&self) -> (B::P, usize);
    ///route by `k` from the current node to the terminal (leaf) under `k`; returns the
    ///terminal's vaddr (the walker is left positioned on it). `None` if at-end.
    fn walk_to(&mut self, k: &<B::T as Node>::K) -> Option<B::P>;
    ///consume the walker, yielding its inner cursor (positioned where `walk_to` left
    ///it). the cursor holds the block borrow + phys; the caller extracts a ref from
    ///the (concrete) cursor — keeping lifetime-proving ref extraction off the generic
    ///trait and with the consumer, where the cursor type is concrete.
    fn current_into(self) -> C;
}

///mut walker. returned `&mut B::T` are tied to the `&mut self` call borrow
///(no streaming iterator). requires `C: CursorMut` (which implies `C: Cursor`).
///`TreeWalker` is a supertrait: navigation is inherited from the shared impl, so
///the mut `Probe` gets `descend`/`ascend`/`walk_to`/`walk_into` for free — only
///the mut surface lives here.
pub trait TreeWalkerMut<'block, 'walker, O, B, C>: TreeWalker<'block, 'walker, O, B, C>
where
    O: Ordering,
    B: TreeBlockMut<'block>,
    B::T: Node,
    'block: 'walker,
    C: CursorMut<'walker, B::T, B::P>,
{
    fn current_mut(&mut self) -> Option<&mut B::T>;
    ///place `node` as child slot `i` of the current inode (placement + fixup + wiring)
    ///and return the new child's vaddr (so a split can `get_disjoint` into it).
    fn insert_child(&mut self, k: <B::T as Node>::K, i: usize, node: B::T) -> B::P;
    ///remove child[idx] from the current inode + its bounding separator, and FREE the
    ///child's block slot. a `Some` orphan is a fixup landmine — a later slide whose run
    ///covers its phys runs `fixup` on it (stale `parent`, parent no longer lists it →
    ///panic), so the slot must become a `None` gap now. returns the separator: the merge
    ///driver inserts it into the kept node for an internal merge, drops it for a leaf
    ///merge. the driver must move the child's contents into the kept node BEFORE this.
    fn remove_child(&mut self, child: usize) -> (B::T,OpenSlot);
    fn split_child(&mut self, child: usize, ptr: B::P)
    where
        B::T: SplittableNode<<B::T as Node>::K>;
    //move current to an open position
    fn swap_none(&mut self, other: OpenSlot);
    ///run-parent-fixup for a pending slide `ns` — rewrite each moved node's stale
    ///parent→child pointer (and its hoisted `parent` field if the parent also moved)
    ///BEFORE the slide is applied. position-neutral. the impl is walker-specific: a
    ///stackless `Probe` vseeks around the run (vaddrs are stable pre-slide); a stackful
    ///walker would walk the run as a traversal.
    fn fixup(&mut self, ns: &NoneSlide);
}