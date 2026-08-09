use std::marker::PhantomData;

use crate::block_cursor::{BlockCursor, Cursor, CursorMut};
use crate::block::{BlockMutTrait, BlockTrait};
use crate::index::BlockIndex;
use crate::node::{D, HasParent, INode, LNode, Node, OrphanUnionNode, SplittableNode, UnionNode};
use crate::store::VecStore;
use crate::alloc_strat::Uniform;
use crate::tree_block::{TreeBlock, TreeBlockMut};
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
    _p:     PhantomData<P>,
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

impl<K: C, V: C, P: BlockIndex> Default for BINode<K, V, P> {
    fn default() -> Self {
        Self { keys: TinyArray::new(), children: TinyArray::new(), _v: PhantomData }
    }
}

impl<K: C, V: C, P: BlockIndex> Default for BLNode<K, V, P> {
    fn default() -> Self {
        Self { keys: TinyArray::new(), values: TinyArray::new(), _p: PhantomData }
    }
}

//fresh tree is a leaf (height 0); the orphan's inode field is left uninit.
impl<K: C, V: C, P: BlockIndex> Default for BNode<K, V, P> {
    fn default() -> Self {
        Self { orphan: OrphanUnionNode { lnode: BLNode::default() }, parent: P::MIN }
    }
}

//internal split: children.len() = keys.len()+1. `mid = child_count/2`; left keeps
//children[0..mid] + keys[0..mid-1), right gets children[mid..] + keys[mid..], the
//separator keys[mid-1] goes up.
impl<K: C, V: C, P: BlockIndex> SplittableNode<K> for BINode<K, V, P> {
    fn split_into(&mut self, blank: &mut Self) -> K {
        let n = self.children.len();
        debug_assert!(n >= 2, "split_into: need >=2 children");
        let mid = n / 2;
        let sep = *self.keys.get(mid - 1);
        for _ in 0..(n - mid) {
            blank.children.push(self.children.remove(mid));
        }
        for _ in 0..(n - 1 - mid) {
            blank.keys.push(self.keys.remove(mid));
        }
        self.keys.remove(mid - 1);
        sep
    }
}

//leaf split: separator = first key of the right half (keys[mid]).
impl<K: C, V: C, P: BlockIndex> SplittableNode<K> for BLNode<K, V, P> {
    fn split_into(&mut self, blank: &mut Self) -> K {
        let n = self.keys.len();
        debug_assert!(n >= 2, "split_into: need >=2 keys");
        let mid = n / 2;
        let sep = *self.keys.get(mid);
        for _ in 0..(n - mid) {
            blank.keys.push(self.keys.remove(mid));
            blank.values.push(self.values.remove(mid));
        }
        sep
    }
}

///b+tree `TreeBlock` meta is its (uniform) height. discriminates inode (depth <
/// height) from lnode (depth == height); the block stores nodes in pre-order, so
/// `go_next`/`go_prev` are block-slot next/prev.
impl<'block, 'walker, K, V, P, B>
    TreeWalker<'block, 'walker, PreOrder, B, BlockCursor<'block, 'walker, B, &'walker B>>
    for Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker B>, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C + Ord,
    V: C,
    P: BlockIndex,
    BNode<K, V, P>: HasParent<P>,
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
            .map(|node| *unsafe { node.orphan.inode.children.get(idx) })?;
        self.depth += 1;
        self.cursor.seek(next)
    }

    fn descend_right(&mut self, times: usize) -> Option<(&B::T, usize)> {
        let mut count = 0;
        while count < times && self.depth < self.meta.0 {
            let n = match self.cursor.current() {
                Some(c) => unsafe { c.orphan.inode.children.len() },
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
                Some(c) => unsafe { c.orphan.inode.children.len() },
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
        //parent is a hoisted field on the wrapper — kind-free, no union access.
        let parent = self.cursor.current().map(|node| node.parent())?;
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

    fn walk_to(&mut self, k: &K) -> Option<P> {
        while self.depth < self.meta.0 {
            let idx = {
                let cur = self.cursor.current()?;
                unsafe { cur.orphan.inode.try_route(k) }.unwrap_or(0)
            };
            self.descend(idx)?;
        }
        self.cursor.address()
    }

    fn walk_into(mut self, k: &K) -> Option<&'walker B::T> {
        let lv = self.walk_to(k)?;
        let (block, _) = self.cursor.into_parts();
        Some(block.vget(lv))
    }
}

impl<'block, 'walker, K, V, P, B>
    TreeWalkerMut<'block, 'walker, PreOrder, B, BlockCursor<'block, 'walker, B, &'walker mut B>>
    for Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker mut B>, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C + Ord,
    V: C,
    P: BlockIndex,
    BNode<K, V, P>: HasParent<P>,
{
    fn new_mut(height: usize, block: &'walker mut B) -> Self {
        let root = block.root();
        Self {
            cursor: BlockCursor::new_at(block, root),
            depth: 0,
            meta: Height(height as u64),
            _l: PhantomData,
        }
    }

    fn current_mut(&mut self) -> Option<&mut <B>::T> {
        self.cursor.current_mut()
    }

    fn walk_to(&mut self, k: &K) -> Option<P> {
        while self.depth < self.meta.0 {
            let next = {
                let cur = self.cursor.current()?;
                let idx = unsafe { cur.orphan.inode.try_route(k) }.unwrap_or(0);
                *unsafe { cur.orphan.inode.children.get(idx) }
            };
            self.cursor.seek(next);
            self.depth += 1;
        }
        self.cursor.address()
    }

    fn walk_into(mut self, k: &K) -> Option<&'walker mut B::T> {
        let lv = self.walk_to(k)?;
        let (block, _) = self.cursor.into_parts();
        Some(block.vget_mut(lv))
    }

    ///place `node` as child slot `i` of the current inode and wire the parent.
    ///the block stores nodes in pre-order, so the new node (root of its subtree)
    ///is placed AFTER its pre-order predecessor: the parent itself for i==0
    //(parent precedes c[0]'s subtree), or the rightmost desc of child[i-1] for
    //i>0 (last node of the left sibling's subtree). the slide `find_slot` opens
    //shifts in-run nodes — their vaddrs change, so each moved node's parent
    //pointer is rewritten (kind-free via the hoisted parent field) before the
    //slide is applied. the parent is re-resolved after (it may move).
    fn insert_child(&mut self, k: <<B>::T as Node>::K, i: usize, node: B::T) {
        debug_assert!(self.depth < self.meta.0, "insert_child: cursor not on an inode");
        let parent_v = self.cursor.address().expect("insert_child: cursor at-end");
        let parent_depth = self.depth;

        //navigate to the pre-order predecessor of the new child's slot. the block
        //stores nodes in pre-order, so the new child (root of its subtree) inserts
        //AFTER its predecessor: the parent itself for i==0 (parent precedes c[0]'s
        //subtree), or the rightmost desc of child[i-1] for i>0 (last node of the left
        //sibling's subtree).
        if i > 0 {
            let mut cv = *unsafe {
                self.cursor
                    .current()
                    .expect("insert_child: parent gone")
                    .orphan
                    .inode
                    .children
                    .get(i - 1)
            };
            self.cursor.seek(cv);
            self.depth += 1;
            while self.depth < self.meta.0 {
                let n = unsafe {
                    self.cursor
                        .current()
                        .expect("insert_child: anchor walk")
                        .orphan
                        .inode
                        .children
                        .len()
                };
                if n == 0 {
                    break;
                }
                cv = *unsafe {
                    self.cursor
                        .current()
                        .expect("insert_child: anchor walk")
                        .orphan
                        .inode
                        .children
                        .get(n - 1)
                };
                self.cursor.seek(cv);
                self.depth += 1;
            }
        }
        //i==0: anchor is the parent (cursor already on it).
        let anchor_phys = self.cursor.position().expect("insert_child: no anchor");

        let root_phys = self.cursor.root_phys();
        let found = self.cursor.find_slot(anchor_phys, true, Some(root_phys));
        let ns = found.slide.expect("insert_child: block exhausted");
        let delta = ns.delta;
        let lo = ns.from.min(ns.to);
        let hi = ns.from.max(ns.to);

        //run-parent-fixup, before the slide. process from the None source (`from`)
        //toward the opened slot (`to`): a node's new vaddr then can't collide with a
        //sibling's still-stale old vaddr during the search-by-old-vaddr below.
        let mut p = if delta > 0 { hi.checked_sub(1) } else if delta < 0 { Some(lo + 1) } else { None };
        while let Some(cur) = p {
            if self.cursor.slot_occupied(cur) {
                let old_v = self.cursor.p2v(cur);
                let new_v = self.cursor.p2v(cur.wrapping_add(delta as usize));
                self.cursor.seek(old_v);
                let parent_v = self.cursor.current().expect("insert_child: moved").parent();
                let parent_phys = self.cursor.v2p(parent_v);
                //parent moved iff its phys is in the run (it can't be `from` — that's None).
                let parent_moved = parent_phys != ns.from && lo <= parent_phys && parent_phys <= hi;
                //parent→child: rewrite the stale child pointer old_v → new_v.
                self.cursor.seek(parent_v);
                let par = self.cursor.current_mut().expect("insert_child: parent");
                let mut matched = false;
                unsafe {
                    let ch = &mut par.orphan.inode.children;
                    let n = ch.len();
                    for j in 0..n {
                        if *ch.get(j) == old_v {
                            *ch.get_mut(j) = new_v;
                            matched = true;
                            break;
                        }
                    }
                }
                debug_assert!(matched, "insert_child: parent missing moved child");
                //child→parent: if this node's parent also moved, its hoisted `parent`
                //field (still the pre-slide vaddr) is stale — repoint at the parent's
                //post-slide vaddr. the root never moves (pinned, outside the run), so a
                //node whose parent is the root is left untouched.
                if parent_moved {
                    let parent_new_v = self.cursor.p2v(parent_phys.wrapping_add(delta as usize));
                    self.cursor.seek(old_v);
                    self.cursor.current_mut().expect("insert_child: moved").set_parent(parent_new_v);
                }
            }
            p = if delta > 0 {
                if cur > lo { Some(cur - 1) } else { None }
            } else {
                if cur < hi { Some(cur + 1) } else { None }
            };
        }

        //re-resolve the pin: a grow inside find_slot remaps the root's phys (vaddr
        //stable), so the pre-grow root_phys is stale for the slide.
        let root_phys = self.cursor.root_phys();
        let opened = self.cursor.slide_none(ns, Some(root_phys));

        //the parent may have moved in the slide; re-resolve its vaddr.
        let parent_phys = self.cursor.v2p(parent_v);
        let parent_v_now = if delta != 0
            && parent_phys != ns.from
            && lo <= parent_phys
            && parent_phys <= hi
        {
            self.cursor.p2v(parent_phys.wrapping_add(delta as usize))
        } else {
            parent_v
        };

        //the new node's parent is this parent (the caller can't know the post-slide
        //vaddr); set it before placing.
        let mut node = node;
        node.set_parent(parent_v_now);
        let new_phys = self.cursor.insert(node, opened);
        let new_v = self.cursor.p2v(new_phys);

        //wire the new child pointer + separator key into the parent.
        self.cursor.seek(parent_v_now);
        if let Some(par) = self.cursor.current_mut() {
            unsafe {
                par.orphan.inode.children.insert_at(i, new_v);
                par.orphan.inode.keys.insert_at(i, k);
            }
        }
        self.depth = parent_depth;
    }

    fn remove_child(&mut self, child: usize) -> Option<(<<B>::T as Node>::K, <B>::P)> {
        let node = self.cursor.current_mut()?;
        debug_assert!(self.depth < self.meta.0);
        Some(unsafe {
            let p = node.orphan.inode.children.remove(child);
            let k = node.orphan.inode.keys.remove(child);
            (k, p)
        })
    }

    //needs the clone-based split machinery (two &mut into one block); see SPLIT_PLAN.
    fn split_child(&mut self, child: usize, ptr: <B>::P)
    where B::T: SplittableNode<<B::T as Node>::K> {
        let _ = (child, ptr);
        todo!("split_child: clone-split driver not yet at this tier")
    }

    //relocate the tracked node into the `None` at `other`; cursor follows its data.
    fn swap_none(&mut self, other: crate::block::OpenSlot) {
        if let Some(src) = self.cursor.position() {
            self.cursor.swap_open(src, other);
        }
    }

    //current node is the parent of the slid nodes: rewrite each (old_v,new_v) child
    //pointer it still holds.
    fn fixup_stack(&mut self, fixup: &[(<B>::P, <B>::P)]) {
        if let Some(node) = self.cursor.current_mut() {
            debug_assert!(self.depth < self.meta.0);
            unsafe {
                let children = &mut node.orphan.inode.children;
                let n = children.len();
                for (old, new) in fixup {
                    for j in 0..n {
                        if *children.get(j) == *old {
                            *children.get_mut(j) = *new;
                            break;
                        }
                    }
                }
            }
        }
    }
}

//concrete b+ tree map over a single pre-order `Uniform` block. the block stores
//`BNode`s (leaves at depth == height, inodes below); this tier routes by key to the
//leaf and edits it in place — no block-slot movement, no slide/fixup (those live at
//the walker tier, exercised by the split driver). splits/merges are not wired yet;
//`insert` panics on a full leaf.
type MapBlock<K, V, P, const CAP: usize> = TreeBlock<
    'static,
    BNode<K, V, P>,
    P,
    Uniform<PreOrder>,
    VecStore<BNode<K, V, P>, CAP>,
    PreOrder,
    Height,
>;

pub struct BTreeMap<K, V, P = u16, const CAP: usize = 4096>
where
    K: D + Copy + Ord,
    V: D + Copy,
    P: BlockIndex,
{
    block: MapBlock<K, V, P, CAP>,
    len: usize,
}

//leaf key capacity: TinyArray<_, 15> == Node::DEGREE - 1.
const LEAF_MAX: usize = 15;

impl<K, V, P, const CAP: usize> BTreeMap<K, V, P, CAP>
where
    K: D + Copy + Ord,
    V: D + Copy,
    P: BlockIndex,
{
    pub fn new() -> Self {
        Self { block: <MapBlock<K, V, P, CAP> as BlockMutTrait>::new(), len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        let w = Probe::new(self.block.meta().0 as usize, &self.block);
        let leaf = unsafe { &w.walk_into(k)?.orphan.lnode };
        let n = leaf.keys.len();
        for i in 0..n {
            if leaf.keys.get(i) == k {
                return Some(leaf.values.get(i));
            }
        }
        None
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let h = self.block.meta().0 as usize;
        let w = Probe::new_mut(h, &mut self.block);
        let leaf = unsafe { &mut w.walk_into(k)?.orphan.lnode };
        let n = leaf.keys.len();
        for i in 0..n {
            if leaf.keys.get(i) == k {
                return Some(leaf.values.get_mut(i));
            }
        }
        None
    }

    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        if self.block.len() == 0 {
            let phys = self.block.insert_root(BNode::default());
            self.block.set_root(self.block.p2v(phys));
        }
        let h = self.block.meta().0 as usize;
        let w = Probe::new_mut(h, &mut self.block);
        let leaf = unsafe { &mut w.walk_into(&k)?.orphan.lnode };
        let n = leaf.keys.len();
        for i in 0..n {
            if leaf.keys.get(i) == &k {
                return Some(std::mem::replace(leaf.values.get_mut(i), v));
            }
        }
        if n >= LEAF_MAX {
            todo!("BTreeMap::insert: leaf split not yet implemented");
        }
        leaf.insert(k, v);
        self.len += 1;
        None
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let h = self.block.meta().0 as usize;
        let w = Probe::new_mut(h, &mut self.block);
        let leaf = unsafe { &mut w.walk_into(k)?.orphan.lnode };
        let n = leaf.keys.len();
        for i in 0..n {
            if leaf.keys.get(i) == k {
                let (_, val) = leaf.remove(i);
                self.len -= 1;
                return Some(val);
            }
        }
        None
    }
}

impl<K, V, P, const CAP: usize> Default for BTreeMap<K, V, P, CAP>
where
    K: D + Copy + Ord,
    V: D + Copy,
    P: BlockIndex,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/btree.rs"]
mod tests;