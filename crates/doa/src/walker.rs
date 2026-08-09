use std::marker::PhantomData;

use crate::block::OpenSlot;
use crate::block_cursor::{Cursor, CursorMut};
use crate::node::{Node, SplittableNode};
use crate::tree_block::TreeBlockMut;
use crate::Ordering;

///use as meta on treeblock for fixed height trees (like b+ trees or S trees)
#[derive(Default)]
pub struct Height(pub u64);

pub struct Walker<'block, B, C>
where
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub(crate) cursor: C,
    pub(crate) stack: Vec<(B::P, usize)>,
    _l: PhantomData<&'block ()>,
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
///(no streaming iterator). requires `C: Cursor`.
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
    ///returns current and new depth
    fn descend_right(&mut self, times: usize) -> Option<(&B::T, usize)>;
    ///returns how many times walker descended.
    fn descend_left(&mut self, times: usize) -> Option<(&B::T, usize)>;
    fn ascend(&mut self) -> Option<&B::T>;
    fn depth(&self) -> usize;
    fn new(height: usize, block: &'walker B) -> Self;
    fn position(&self) -> (B::P, usize);
    ///route by `k` from the current node to the terminal (leaf) under `k`; returns the
    ///terminal's vaddr (the walker is left positioned on it). `None` if at-end.
    fn walk_to(&mut self, k: &<B::T as Node>::K) -> Option<B::P>;
    ///consume: route by `k` to the terminal leaf, yielding it as a ref tied to the
    ///block borrow (`'walker`) so it outlives the walker. `None` if at-end.
    fn walk_into(self, k: &<B::T as Node>::K) -> Option<&'walker B::T>;
}

///mut walker. returned `&mut B::T` are tied to the `&mut self` call borrow
///(no streaming iterator). requires `C: CursorMut` (which implies `C: Cursor`,
///so a mut walker may also drive the read trait).
pub trait TreeWalkerMut<'block, 'walker, O, B, C>
where
    O: Ordering,
    B: TreeBlockMut<'block>,
    B::T: Node,
    'block: 'walker,
    C: CursorMut<'walker, B::T, B::P>,
{
    fn new_mut(height: usize, block: &'walker mut B) -> Self;
    fn current_mut(&mut self) -> Option<&mut B::T>;
    ///route by `k` from the current node to the terminal (leaf) under `k`; returns the
    ///terminal's vaddr (the walker is left positioned on it). `None` if at-end.
    fn walk_to(&mut self, k: &<B::T as Node>::K) -> Option<B::P>;
    ///consume: route by `k` to the terminal leaf, yielding it as a mut ref tied to
    ///the block borrow (`'walker`) so it outlives the walker. `None` if at-end.
    fn walk_into(self, k: &<B::T as Node>::K) -> Option<&'walker mut B::T>;
    fn insert_child(&mut self, k: <B::T as Node>::K, i: usize, node: B::T);
    fn remove_child(&mut self, child: usize) -> Option<(<B::T as Node>::K, B::P)>;
    fn split_child(&mut self, child: usize, ptr: B::P)
    where
        B::T: SplittableNode<<B::T as Node>::K>;
    //move current to an open position
    fn swap_none(&mut self, other: OpenSlot);
    //fixup internal pointers after applying a slide
    fn fixup_stack(&mut self, fixup: &[(B::P, B::P)]);
}