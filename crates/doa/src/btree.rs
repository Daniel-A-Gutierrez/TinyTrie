use std::marker::PhantomData;

use crate::block_cursor::{BlockCursor, Cursor};
use crate::index::BlockIndex;
use crate::node::{D, INode, LNode, Node, SplittableNode, UnionNode};
use crate::tree_block::TreeBlockMut;
use crate::walker::*;
use crate::PreOrder;
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
        Some(self.keys().position(|key| key >= k).unwrap_or(self.keys.len()))
    }

    fn child(&self, child_idx: usize) -> &P {
        &self.children.get(child_idx)
    }

    fn children(&self) -> impl crate::node::DoubleExact<Item = &P> {
        self.children.as_slice().iter()
    }

    fn insert_child(&mut self, child_addr: Self::P, child_key: Self::K) -> usize {
        let pos = self.keys().position(|key| &child_key < key).unwrap_or(self.keys.len());
        self.children.insert_at(pos, child_addr);
        self.keys.insert_at(pos, child_key);
        return pos;
    }

    fn remove_child(&mut self, child_key: &Self::K) -> Option<(Self::K, Self::P)> {
        let pos = self.keys().position(|key| key >= child_key).unwrap_or(self.keys.len());
        let p = self.children.remove(pos);
        let k = self.keys.remove(pos);
        return Some((k, p));
    }
}

impl<K, V, P> LNode<K, V> for BLNode<K, V, P>
where
    K: C + Ord,
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
        let pos = self.keys().position(|key| k < *key).unwrap_or(self.keys.len());
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
        return pos;
    }

    fn remove(&mut self, pos: usize) -> (K, V) {
        (self.keys.remove(pos), self.values.remove(pos))
    }
}

type BNode<K, V, P> = UnionNode<BINode<K, V, P>, BLNode<K, V, P>>;

///b+tree `TreeBlock` meta is its (uniform) height. discriminates inode (depth <
/// height) from lnode (depth == height); the block stores nodes in pre-order, so
/// `go_next`/`go_prev` are block-slot next/prev.
impl<'block, 'walker, K, V, P, B>
    TreeWalker<'block, 'walker, PreOrder, B, BlockCursor<'block, 'walker, B, &'walker B>>
    for Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker B>, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C,
    V: C,
    P: BlockIndex,
{
    fn go_next(&mut self) -> Option<&B::T> {
        self.cursor.next()
    }

    fn go_prev(&mut self) -> Option<&B::T> {
        self.cursor.prev()
    }

    fn descend(&mut self, idx: usize) -> Option<&B::T> {
        if self.depth >= self.meta.0 {
            return None;
        }
        let next = self
            .cursor
            .current()
            .map(|node| *unsafe { node.inode.children.get(idx) })?;
        self.depth += 1;
        self.cursor.seek(next)
    }

    fn descend_right(&mut self, times: usize) -> Option<(&B::T, usize)> {
        let mut count = 0;
        while count < times && self.depth < self.meta.0 {
            let n = match self.cursor.current() {
                Some(c) => unsafe { c.inode.children.len() },
                None => break,
            };
            if n == 0 {
                break;
            }
            self.descend(n - 1);
            count += 1;
        }
        self.cursor.current().map(|c| (c, count))
    }

    fn descend_left(&mut self, times: usize) -> Option<(&B::T, usize)> {
        let mut count = 0;
        while count < times && self.depth < self.meta.0 {
            let n = match self.cursor.current() {
                Some(c) => unsafe { c.inode.children.len() },
                None => break,
            };
            if n == 0 {
                break;
            }
            self.descend(0);
            count += 1;
        }
        self.cursor.current().map(|c| (c, count))
    }

    fn ascend(&mut self) -> Option<&B::T> {
        if self.depth == 0 {
            return None;
        }
        let parent = self.cursor.current().map(|node| {
            if self.depth < self.meta.0 {
                unsafe { node.inode.parent }
            } else {
                unsafe { node.lnode.parent }
            }
        })?;
        self.depth -= 1;
        self.cursor.seek(parent)
    }

    fn depth(&self) -> usize {
        self.depth as usize
    }

    fn new(height: usize, block: &'walker B) -> Self {
        Self {
            cursor: BlockCursor::new_at(block, block.root()),
            depth: 0,
            meta: Height(height as u64),
            _l: PhantomData,
        }
    }

    fn position(&self) -> (B::P, usize) {
        (self.cursor.address().unwrap_or(P::MIN), self.depth as usize)
    }
}

impl<'block, 'walker, K, V, P, B>
    TreeWalkerMut<'block, 'walker, PreOrder, B, BlockCursor<'block, 'walker, B, &'walker mut B>>
    for Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker mut B>, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C,
    V: C,
    P: BlockIndex,
{
    fn new_mut(height: usize, block: &'walker mut B) -> Self {
        todo!()
    }

    fn current_mut(&mut self) -> Option<&mut <B>::T> {
        todo!()
    }

    fn insert_child(self, k: <<B>::T as Node>::K, i: usize, ptr: <B>::P) -> Self {
        todo!()
    }

    fn remove_child(&mut self, child: usize) -> Option<(<<B>::T as Node>::K, <B>::P)> {
        todo!()
    }

    fn split_child(self, child: usize, ptr: <B>::P) -> Self
    where B::T: SplittableNode<<B::T as Node>::K> {
        todo!()
    }

    fn swap_none(self, other: crate::block::OpenSlot) -> Self {
        todo!()
    }

    fn fixup_stack(self, fixup: &[(<B>::P, <B>::P)]) -> Self {
        todo!()
    }
}