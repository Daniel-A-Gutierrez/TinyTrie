use std::marker::PhantomData;

use crate::block::OpenSlot;
use crate::index::BlockIndex;
use crate::node::{INode, LNode, Node, SplittableNode, UnionNode};
use crate::store::{NoneSlide, Store};
use crate::tree_block::{TreeBlock, TreeBlockMut};
use crate::{Ordering, PreOrder};

///use as meta on treeblock for fixed height trees (like b+ trees or S trees)
pub struct Height(u64);

pub struct Walker<'block, 'walker, B>
where
    B: TreeBlockMut<'block>,
    B::T: Node,
{
    pub(crate) block: &'walker B,
    pub(crate) stack: Vec<(B::P, usize)>,
}

//stackless, no alloc.
pub struct Probe<'block, 'probe, B>
where
    B: TreeBlockMut<'block>,
    B::T: Node,
    'block: 'probe,
{
    pub(crate) cursor: B::Cursor<'probe>,
}

pub trait TreeWalker<'block, 'walker, O: Ordering, B: TreeBlockMut<'block, T: Node>>
where 'block: 'walker
{
    ///returns current after moving to next in ordering
    fn go_next<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b;
    ///returns current after moving to prev in ordering
    fn go_prev<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b;
    ///returns current after moving to child at idx
    fn descend<'b>(&'b mut self, idx: usize) -> Option<&'b B::T>
    where 'walker: 'b;
    ///returns current and new depth
    fn descend_right<'b>(&'b mut self, times: usize) -> Option<(&'b B::T, usize)>
    where 'walker: 'b;
    ///returns how many times walker descended.
    fn descend_left<'b>(&'b mut self, times: usize) -> Option<(&'b B::T, usize)>
    where 'walker: 'b;
    fn ascend<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b;
    fn depth(&self) -> usize;
    fn new(height: usize, block: &B) -> Self;
    fn position(&self) -> (B::P, usize);
}

pub trait TreeWalkerMut<'block, 'walker, O: Ordering, B: TreeBlockMut<'block>>:
    TreeWalker<'block, 'walker, O, B>
where
    B::T: Node,
    'block: 'walker,
{
    fn new_mut(height: usize, block: &'walker mut B) -> Self;
    fn current_mut<'b>(&'b mut self) -> &'b mut B::T
    where 'walker: 'b;
    fn insert_child(&mut self, k: <B::T as Node>::K, i: usize, ptr: B::P);
    fn remove_child(&mut self, child: usize) -> Option<(<B::T as Node>::K, B::P)>;
    fn split_child(&mut self, child: usize, ptr: B::P)
    where B::T: SplittableNode<<B::T as Node>::K>;
    //move current to an open position
    fn swap_none(&mut self, other: OpenSlot);
    //fixup internal pointers after applying a slide
    fn fixup_stack(&mut self, fixup: &[(B::P, B::P)]);
}

// the following impls are templates, they cannot work with only Node as the bound.
// impl<'block,'walker, B : TreeBlockMut<'block>>
// TreeWalker<'block,'walker, PreOrder, B>
// for Walker<'block,'walker,B>
// where
//     B::T : Node ,
//     'block : 'walker
// {
//     ///returns current after moving to next in ordering
//     fn go_next<'b>(&'b mut self)->Option<&'b B::T> where 'walker : 'b {todo!()}
//     ///returns current after moving to prev in ordering
//     fn go_prev<'b>(&'b mut self)->Option<&'b B::T> where 'walker : 'b {todo!()}
//     ///returns current after moving to child at idx
//     fn descend<'b>(&'b mut self, idx : usize)-> Option<&'b B::T> where 'walker : 'b {todo!()}
//     ///returns current and new depth
//     fn descend_right<'b>(&'b mut self, times : usize)-> Option<(&'b B::T,usize)> where 'walker : 'b {todo!()}
//     ///returns how many times walker descended.
//     fn descend_left<'b>(&'b mut self, times : usize)->  Option<(&'b B::T,usize)> where 'walker : 'b {todo!()}
//     fn ascend<'b>(&'b mut self) -> Option<&'b B::T> where 'walker : 'b {todo!()}
//     fn depth(&self) -> usize {todo!()}
//     fn new(height : usize, block : &B) -> Self {todo!()}
//     fn position(&self) -> (B::P,usize) {todo!()}
// }

// impl<'block,'walker, B : TreeBlockMut<'block>>
// TreeWalkerMut<'block,'walker, PreOrder, B>
// for Walker<'block,'walker,B>
// where
//     B::T : Node ,
//     'block : 'walker
// {
//     fn new_mut(height : usize, block : &'walker mut B) -> Self {todo!()}
//     fn current_mut<'b>(&'b mut self) -> &'b mut B::T where 'walker : 'b {todo!()}
//     fn insert_child(&mut self, k : <B::T as Node>::K, i : usize, ptr : B::P) {todo!()}
//     fn remove_child(&mut self, child : usize) -> Option<(<B::T as Node>::K, B::P)> {todo!()}
//     fn split_child(&mut self, child : usize, ptr : B::P) where B::T : SplittableNode<<B::T as Node>::K> {todo!()}
//     //move current to an open position
//     fn swap_none(&mut self, other : OpenSlot) {todo!()}
//     //fixup internal pointers after applying a slide
//     fn fixup_stack(&mut self, fixup : &[(B::P,B::P)]) {todo!()}
// }
