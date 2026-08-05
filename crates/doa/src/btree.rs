use std::marker::PhantomData;

use crate::block::Cursor;
use crate::block::OpenSlot;
use crate::index::BlockIndex;
use crate::node::{D, INode, LNode, Node, SplittableNode, UnionNode};
use crate::tree_block::{TreeBlock, TreeBlockMut};
use crate::walker::*;
use crate::{Ordering, PreOrder};
use arrays::tiny_array::TinyArray;

trait C: D + Copy {}
impl<T> C for T where T: D + Copy {}

#[derive(Copy, Clone)]
struct BINode<K, V, P>
where
    K: C,
    V: C,
    P: BlockIndex,
{
    keys:     TinyArray<K, 15>,
    children: TinyArray<P, 16>,
    parent:   P,
    _v:       PhantomData<V>,
}
#[derive(Copy, Clone)]
struct BLNode<K, V, P>
where
    K: C,
    V: C,
    P: BlockIndex,
{
    keys:   TinyArray<K, 15>,
    values: TinyArray<V, 15>,
    parent: P,
}

impl<K, V, P> Node for BINode<K, V, P>
where
    K: C,
    V: C,
    P: BlockIndex,
{
    type K = K;
    type P = P;
    type V = V;
    const DEGREE: usize = 16;
    const HAS_PARENT: bool = true;
    fn parent(&self) -> Option<Self::P> {
        return Some(self.parent);
    }
    fn update_parent(&mut self, p: Self::P) {
        self.parent = p;
    }
}

impl<K, V, P> Node for BLNode<K, V, P>
where
    K: C,
    V: C,
    P: BlockIndex,
{
    type K = K;
    type P = P;
    type V = V;
    const DEGREE: usize = 16;
    const HAS_PARENT: bool = true;
    fn parent(&self) -> Option<Self::P> {
        return Some(self.parent);
    }
    fn update_parent(&mut self, p: Self::P) {
        self.parent = p;
    }
}

impl<K, V, P> INode for BINode<K, V, P>
where
    K: C + Ord,
    V: C,
    P: BlockIndex,
{
    fn keys(&self) -> impl crate::node::DoubleExact<Item = &Self::K> {
        self.keys.as_slice().iter()
    }

    fn try_route(&self, k: &Self::K) -> Option<usize> {
        Some(self.keys().position(|key| key >= k).unwrap_or_else(self.keys.len()))
    }

    fn child(&self, child_idx: usize) -> &P {
        &self.children.get(child_idx)
    }

    fn children(&self) -> impl crate::node::DoubleExact<Item = &P> {
        self.children.as_slice().iter()
    }

    fn insert_child(&mut self, child_addr: Self::P, child_key: Self::K) -> usize {
        let pos = self.keys().position(|key| &child_key < key).unwrap_or_else(self.keys.len());
        self.children.insert_at(pos, child_addr);
        self.keys.insert_at(pos, child_key);
        return pos;
    }

    fn remove_child(&mut self, child_key: &Self::K) -> Option<(Self::K, Self::P)> {
        let pos = self.keys().position(|key| key >= child_key).unwrap_or_else(self.keys.len());
        let p = self.children.remove(pos);
        let k = self.keys.remove(pos);
        return Some((k, p));
    }
}

impl<K, V, P> LNode<K, V> for BLNode<K, V, P>
where
    K: C,
    V: C,
    P: BlockIndex,
{
    fn values(&self) -> impl crate::node::DoubleExact<Item = &V> {
        return self.values.as_slice().iter();
    }

    fn pairs(&self) -> impl crate::node::DoubleExact<Item = (&K, &V)> {
        return self.keys().zip(self.values());
    }

    fn keys(&self) -> impl crate::node::DoubleExact<Item = &K> {
        return self.keys.as_slice().iter();
    }

    fn insert(&mut self, k: K, v: V) -> usize {
        let pos = self.keys().position(|key| k < key).unwrap_or_else(self.keys.len());
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
        return pos;
    }

    fn remove(&mut self, pos: usize) -> (K, V) {
        (self.keys.remove(pos), self.values.remove(pos))
    }
}

type BNode<K, V, P> = UnionNode<BINode<K, V, P>, BLNode<K, V, P>>;

///for a b+tree treeblock's meta is its (uniform) height.
impl<'block, 'walker, K, V, P, B> TreeWalker<'block, 'walker, PreOrder, B>
    for Probe<'block, 'walker, B>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = usize>,
    K: C,
    V: C,
    P: BlockIndex,
{
    ///returns current after moving to next in ordering
    fn go_next<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b {
        //at leaf
        self.cursor.next();
        self.cursor.current()
    }
    ///returns current after moving to prev in ordering
    fn go_prev<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b {
        self.cursor.prev();
        self.cursor.current();
    }
    ///returns current after moving to child at idx
    fn descend<'b>(&'b mut self, idx: usize) -> Option<&'b B::T>
    where 'walker: 'b {
        let node = self.cursor.current().unwrap();
        if self.depth < self.cursor {}
    }
    ///returns current and new depth
    fn descend_right<'b>(&'b mut self, times: usize) -> Option<(&'b B::T, usize)>
    where 'walker: 'b {
        todo!()
    }
    ///returns how many times walker descended.
    fn descend_left<'b>(&'b mut self, times: usize) -> Option<(&'b B::T, usize)>
    where 'walker: 'b {
        todo!()
    }
    fn ascend<'b>(&'b mut self) -> Option<&'b B::T>
    where 'walker: 'b {
        todo!()
    }
    fn depth(&self) -> usize {
        todo!()
    }
    fn new(height: usize, block: &B) -> Self {
        todo!()
    }
    fn position(&self) -> (B::P, usize) {
        todo!()
    }
}

impl<'block, 'walker, K, V, P, B> TreeWalkerMut<'block, 'walker, PreOrder, B>
    for Probe<'block, 'walker, B>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = usize>,
    K: C,
    V: C,
    P: BlockIndex,
{
    fn new_mut(height: usize, block: &'walker mut B) -> Self {
        todo!()
    }
    fn current_mut<'b>(&'b mut self) -> &'b mut B::T
    where 'walker: 'b {
        todo!()
    }
    fn insert_child(&mut self, k: <B::T as Node>::K, i: usize, ptr: B::P) {
        todo!()
    }
    fn remove_child(&mut self, child: usize) -> Option<(<B::T as Node>::K, B::P)> {
        todo!()
    }
    fn split_child(&mut self, child: usize, ptr: B::P)
    where B::T: SplittableNode<<B::T as Node>::K> {
        todo!()
    }
    //move current to an open position
    fn swap_none(&mut self, other: OpenSlot) {
        todo!()
    }
    //fixup internal pointers after applying a slide
    fn fixup_stack(&mut self, fixup: &[(B::P, B::P)]) {
        todo!()
    }
}
