use std::marker::PhantomData;

//use crate::block::{OpenSlot, TreeBlock};
use crate::blocks::{BlockTrait, OpenSlot};
use crate::treeblock::*;
use crate::index::BlockIndex;
use crate::metadata::Fixable;
//use crate::node::{Node, SplittableNode};
use crate::store::NoneSlide;
use crate::TreeOrdering;


//default shorthand for stored types, temporary till we care. 
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


pub trait NodeWalkerT<'block, 'walker, B> : Sized + Fixable<B::P>
where 
    B::T: Node + Default,
    B::O : TreeOrdering,
    B : TreeBlock<'block>,
    'block: 'walker, {
    fn from_block(b : &B) -> Self;
    fn block(&self) -> &B;
    fn is_leaf(&self) -> bool;
    fn depth(&self) -> usize;
    fn lookup(&self, k: <B::T as Node>::K) -> usize;
    fn child(&self, idx : usize) -> <B::T as Node>::P;
    fn ascend(&mut self)->&B::T;
    fn descend(&mut self, child_idx : usize)-> &B::T;
    fn position(&self) -> usize;
    fn current(&self) -> &B::T;
}

pub trait NodeWalkerMutT<'block, 'walker, B> : NodeWalkerT<'block,'walker,B> 
where 
    B::T: Node + Default,
    B::O : TreeOrdering,
    B : TreeBlock<'block>,
    'block: 'walker, {
    type WD: Fixable<B::P>;
    fn from_block_mut(b: &mut B) -> Self;
    fn block_mut(&mut self) -> &mut B;
    fn current_mut(&mut self) -> &mut B;
    fn into_parts(self) -> (Self::WD, &'walker mut B);
}

struct TreeWalker<NW,O> {
    nw : NW,
    _o : PhantomData<O>
}

pub trait TreeWalkerT<'block, 'walker, NW, B>
where
    NW : NodeWalkerMutT<'block,'walker,B>,
    B::O : TreeOrdering,
    B: TreeBlock<'block>,
    B::T: Node + Default,
    'block: 'walker,
{
    fn prev(&mut self) -> Option<&B>;
    fn next(&mut self) -> Option<&B>;
    fn prev_mut(&mut self) -> Option<&mut B>;
    fn next_mut(&mut self) -> Option<&mut B>;
    //position self at boundary of child subtree. return value indicates before (false) after(true) the first/last position in the subtree.
    fn boundary(&mut self, child_idx : usize, after : bool) -> bool;
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