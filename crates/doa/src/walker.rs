//use crate::block::{OpenSlot, TreeBlock};
use crate::blocks::{OpenSlot};
use crate::treeblock::*;
use crate::index::BlockIndex;
use crate::metadata::Fixable;
//use crate::node::{Node, SplittableNode};
use crate::store::NoneSlide;
use crate::TreeOrdering;


//default shorthand for stored types
pub trait SS: 'static + Sized {}
impl<T> SS for T where T: 'static + Sized {}

pub trait Node {
    type K: SS;
    type V: SS;
    type P: BlockIndex;
    //maximum number of children per node (relevant for in-order ordering)
    const DEGREE: usize;
}

pub trait SplittableNode : Node {
    fn split(&mut self) -> Self;
}

pub trait TreeWalker<'block, 'walker, B> : Sized
where
    B::O : TreeOrdering,
    B: TreeBlock<'block>,
    B::T: Node + Default,
    'block: 'walker,
{
    ///walker-local tracked state (depth for a probe, ancestry for a stackful walker).
    type WD: Fixable<B::P>;
    fn block(&self) -> &B;
    fn go_next(&mut self) -> Option<&B::T>;
    fn go_prev(&mut self) -> Option<&B::T>;
    fn descend(&mut self, idx: usize) -> Option<&B::T>;
    fn descend_right(&mut self, times: usize) -> Option<(&B::T, usize)>;
    fn descend_left(&mut self, times: usize) -> Option<(&B::T, usize)>;
    fn ascend(&mut self) -> Option<&B::T>;
    fn depth(&self) -> usize;
    fn position(&self) -> (B::P, usize);
    fn parent(&self) -> Option<B::P>; //parent addr, index in parent.
    ///route by `k` from the current node to the terminal (leaf) under `k`; returns the
    ///terminal's vaddr. `None` if at-end.
    fn walk_to(&mut self, k: &<B::T as Node>::K) -> Option<B::P>;
    ///consume the walker, yielding its inner cursor.
    fn current_into(self) -> usize;
}

///mut walker. `TreeWalker` is a supertrait: navigation is inherited. only the mut surface
///lives here.
pub trait TreeWalkerMut<'block, 'walker, B>: TreeWalker<'block, 'walker, B>
where
    B::O : TreeOrdering,
    B: TreeBlock<'block>,
    B::T: Node + Default,
    'block: 'walker,
{
    fn current_mut(&mut self) -> Option<&mut B::T>;
    // fn insert_child(&mut self, k: <B::T as Node>::K, i: usize, node: B::T) -> B::P;
    // ///remove child[idx] + its bounding separator, and FREE the child's block slot.
    // fn remove_child(&mut self, child: usize) -> (B::T, OpenSlot);
    // fn split_child(&mut self, child: usize, ptr: B::P)
    // where
    //     B::T: SplittableNode<<B::T as Node>::K>;
    // fn swap_none(&mut self, other: OpenSlot);
    // ///run-parent-fixup for a pending slide `ns` — rewrite each moved node's stale
    // ///parent→child pointer BEFORE the slide is applied. position-neutral.
    // fn fixup(&mut self, ns: &NoneSlide);
    // ///split a full block at the root's median-child subtree boundary. consumes the walker:
    // ///self's block becomes the left half, the returned block is the right half, the
    // ///separator key is returned for the caller to wire both under an arena parent.
    // fn split_tree(self) -> (B, <B::T as Node>::K);
}


pub trait SplitTreeWalker<'block, 'walker, B>: TreeWalkerMut<'block, 'walker, B>
where
    B::O : TreeOrdering,
    B: TreeBlock<'block>,
    B::T: SplittableNode + Default,
    'block: 'walker,
{
    //or something
    fn split_child(&mut self, idx : usize) -> Option<&mut B::T>;
}