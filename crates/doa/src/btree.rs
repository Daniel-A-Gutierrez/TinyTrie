use std::cmp::Ordering;
use std::marker::PhantomData;

use crate::block_cursor::{BlockCursor, Cursor, CursorMut};
use crate::block::{BlockMutTrait, BlockTrait, OpenSlot};
use crate::index::BlockIndex;
use crate::node::{D, HasParent, INode, LNode, Node, OrphanUnionNode, SplittableNode, UnionNode};
use crate::store::{NoneSlide, VecStore};
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

    fn lookup(&self, k: &Self::K) -> Option<(usize, Ordering)> {
        //B+ min-separator: keys[i] = min(child[i+1]). `pos` = first key >= k; the walker
        //routes an equal separator to the right child (`pos + (cmp == Equal)`), which
        //matches the old "first key strictly > k" rule. always Some (height ⇒ a child).
        let keys = self.keys.as_slice();
        let pos = keys.iter().position(|key| key >= k).unwrap_or(keys.len());
        let cmp = if pos < keys.len() { k.cmp(&keys[pos]) } else { Ordering::Greater };
        Some((pos, cmp))
    }

    fn child(&self, child_idx: usize) -> &P {
        &self.children.get(child_idx)
    }

    fn children(&self) -> impl crate::node::DoubleExact<Item = &P> {
        self.children.as_slice().iter()
    }

    //B+ insert: keys[i] = min(child[i+1]). the new child's min (child_key) is the
    //separator that lands between the new child and its left sibling. pos = first
    //key > child_key; insert child_key at keys[pos] and the child at children[pos+1].
    fn insert_child(&mut self, child_key: Self::K, child_addr: Self::P) -> usize {
        debug_assert!(
            self.children.len() == self.keys.len() + 1,
            "insert_child: B+ broken children={} keys={}",
            self.children.len(),
            self.keys.len()
        );
        let pos = self.keys().position(|key| child_key < *key).unwrap_or(self.keys.len());
        self.keys.insert_at(pos, child_key);
        self.children.insert_at(pos + 1, child_addr);
        pos + 1
    }

    //B+ remove: child_key is the min (= separator) of the child to remove. find the
    //separator equal to child_key; remove the child after it and that separator.
    fn remove_child(&mut self, child_key: &Self::K) -> Option<(Self::K, Self::P)> {
        let pos = self.keys().position(|key| key == child_key)?;
        let p = self.children.remove(pos + 1);
        let k = self.keys.remove(pos);
        Some((k, p))
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

    fn lookup(&self, k: &K) -> (usize, Ordering) {
        let keys = self.keys.as_slice();
        let pos = keys.iter().position(|key| key >= k).unwrap_or(keys.len());
        let cmp = if pos < keys.len() { k.cmp(&keys[pos]) } else { Ordering::Greater };
        (pos, cmp)
    }

    fn insert(&mut self, k: K, v: V) -> usize {
        let pos = self.keys().position(|key| k < *key).unwrap_or(self.keys.len());
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
        return pos;
    }

    fn insert_at(&mut self, pos: usize, k: K, v: V) {
        self.keys.insert_at(pos, k);
        self.values.insert_at(pos, v);
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
        debug_assert_eq!(
            self.children.len(),
            self.keys.len() + 1,
            "split_into: B+ invariant broken (children != keys+1)"
        );
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

///variant-dependent fullness: inode (depth < height) full at `children.len() == DEGREE`
///(=16); leaf full at `keys.len() == LEAF_MAX` (=15). `TinyArray::is_full` checks `len == N`.
fn node_full<K: C, V: C, P: BlockIndex>(node: &BNode<K, V, P>, depth: u64, height: u64) -> bool {
    if depth < height {
        unsafe { node.orphan.inode.children.is_full() }
    } else {
        unsafe { node.orphan.lnode.keys.is_full() }
    }
}

///b+tree `TreeBlock` meta is its (uniform) height. discriminates inode (depth <
/// height) from lnode (depth == height); the block stores nodes in pre-order, so
/// `go_next`/`go_prev` are block-slot next/prev.
///
///one generic `TreeWalker` impl over `Cs: Cursor` serves both the shared (`&B`) and
///mut (`&mut B`) `Probe` variants — navigation + `walk_to` touch only `Cursor`
///methods, and `CursorMut: Cursor`. `TreeWalkerMut: TreeWalker` inherits it. the
///cursor is returned via `current_into`; ref extraction (`into_parts` + `get`) stays
///with the consumer, where the cursor is concrete and lifetimes are provable.
impl<'block, 'walker, K, V, P, B, Cs>
    TreeWalker<'block, 'walker, PreOrder, B, Cs>
    for Probe<'block, B, Cs, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C + Ord,
    V: C,
    P: BlockIndex,
    BNode<K, V, P>: HasParent<P>,
    Cs: Cursor<'walker, BNode<K, V, P>, P>,
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
        self.cursor.vseek(next)
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
            if self.descend(n - 1).is_none() {
                break;
            }
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
        self.cursor.vseek(parent)
    }

    fn depth(&self) -> usize {
        self.depth as usize
    }

    fn position(&self) -> (B::P, usize) {
        (self.cursor.address().unwrap_or(P::MIN), self.depth as usize)
    }

    fn walk_to(&mut self, k: &K) -> Option<P> {
        while self.depth < self.meta.0 {
            let child = {
                let cur = self.cursor.current()?;
                //B+ routing: first key >= k, then route an equal separator right.
                let (pos, cmp) = unsafe { cur.orphan.inode.lookup(k).unwrap_unchecked() };
                pos + (cmp == Ordering::Equal) as usize
            };
            self.descend(child)?;
        }
        self.cursor.address()
    }

    fn current_into(self) -> Cs {
        self.cursor
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
    fn current_mut(&mut self) -> Option<&mut <B>::T> {
        self.cursor.current_mut()
    }

    ///run-parent-fixup for a pending slide `ns` (called BEFORE the slide): each node in
    ///the moved run `[lo,hi]` (excl. the `None` at `from`) gets its stale parent→child
    ///pointer rewritten (`old_v → new_v`), and — if its parent also moved — its hoisted
    ///`parent` field repointed at the parent's post-slide vaddr. position-neutral:
    ///vaddrs are stable pre-slide, so the cursor is saved/restored.
    ///Probe impl: vseek around the run by phys arithmetic. the O(DEGREE) child-pointer
    ///scan is the cost of being stackless — a stackful walker over traversal-ordered
    ///storage would walk the run and read parents off its stack (O(1), no scan).
    fn fixup(&mut self, ns: &NoneSlide) {
        let saved = self.cursor.address();
        let delta = ns.delta;
        let lo = ns.from.min(ns.to);
        let hi = ns.from.max(ns.to);
        //process from the None source (`from`) toward the opened slot (`to`): a node's
        //new vaddr then can't collide with a sibling's still-stale old vaddr below.
        let mut p = if delta > 0 { hi.checked_sub(1) } else if delta < 0 { Some(lo + 1) } else { None };
        while let Some(cur) = p {
            if self.cursor.slot_occupied(cur) {
                let old_v = self.cursor.p2v(cur);
                let new_v = self.cursor.p2v(cur.wrapping_add(delta as usize));
                self.cursor.vseek(old_v);
                let parent_v = self.cursor.current().expect("fixup: moved").parent();
                let parent_phys = self.cursor.v2p(parent_v);
                //parent moved iff its phys is in the run (it can't be `from` — that's None).
                let parent_moved = parent_phys != ns.from && lo <= parent_phys && parent_phys <= hi;
                //parent→child: rewrite the stale child pointer old_v → new_v.
                self.cursor.vseek(parent_v);
                let par = self.cursor.current_mut().expect("fixup: parent");
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
                debug_assert!(matched, "fixup: parent missing moved child");
                //child→parent: if this node's parent also moved, its hoisted `parent`
                //field (still the pre-slide vaddr) is stale — repoint at the parent's
                //post-slide vaddr. the root never moves (pinned, outside the run), so a
                //node whose parent is the root is left untouched.
                if parent_moved {
                    let parent_new_v = self.cursor.p2v(parent_phys.wrapping_add(delta as usize));
                    self.cursor.vseek(old_v);
                    self.cursor.current_mut().expect("fixup: moved").set_parent(parent_new_v);
                }
            }
            p = if delta > 0 {
                if cur > lo { Some(cur - 1) } else { None }
            } else {
                if cur < hi { Some(cur + 1) } else { None }
            };
        }
        if let Some(v) = saved { let _ = self.cursor.vseek(v); }
    }

    ///place `node` as child slot `i` of the current inode and wire the parent.
    //the slide `find_slot` opens
    //shifts in-run nodes — their vaddrs change, so each moved node's parent
    //pointer is rewritten (kind-free via the hoisted parent field) before the
    //slide is applied. the parent is re-resolved after (it may move).
    fn insert_child(&mut self, k: <<B>::T as Node>::K, i: usize, node: B::T) -> B::P {
        debug_assert!(self.depth < self.meta.0, "insert_child: cursor not on an inode");
        let parent_v = self.cursor.address().expect("insert_child: cursor at-end");
        let parent_depth = self.depth;

        //navigate to the pre-order predecessor of the new child's slot. 
        //the new child (root of its subtree) inserts
        //AFTER its predecessor: the parent itself for i==0 (parent precedes c[0]'s
        //subtree), or the rightmost desc of child[i-1] for i>0 (last node of the left
        //sibling's subtree).
        if i > 0 {
            let _ = self.descend(i - 1);
            self.descend_right(self.meta.0 as usize - self.depth as usize);
        }
        //i==0: anchor is the parent (cursor already on it).
        let anchor_phys = self.cursor.position().expect("insert_child: no anchor");

        let (new_v, parent_v_now) = self.place_at(anchor_phys, parent_v, node);

        //wire the new child + its B+ separator into the parent. B+: keys[i] =
        //min(child[i+1]); the new child at slot i has min `k` = the separator between
        //child[i-1] and child[i], so the key lands at i-1 (i>0; i==0 underflow insert
        //is not exercised by the split path).
        self.cursor.vseek(parent_v_now);
        if let Some(par) = self.cursor.current_mut() {
            unsafe {
                par.orphan.inode.children.insert_at(i, new_v);
                par.orphan.inode.keys.insert_at(i - 1, k);
            }
        }
        self.depth = parent_depth;
        new_v
    }

    ///removes a leaf node child of an inode. does not merge or shift, cannot remove inodes. 
    fn remove_child(&mut self, child: usize) -> (B::T,OpenSlot) {
        let node = self.cursor.current_mut().expect("Attempted remove child on None");
        assert!(self.depth == self.meta.0 - 1, "Attempted to remove inode");
        //B+: remove child[i] and the separator that bounded it — keys[i-1] for i>0,
        //keys[0] for the underflow child[0] (its right neighbor becomes the new underflow).
        //ideally this would cascade - an inode with no keys is strange - but thats TODO. 
        let p = unsafe {node.orphan.inode.children.remove(child)};
        unsafe {node.orphan.inode.keys.remove(if child == 0 { 0 } else { child - 1 })};
        let phys = self.cursor.v2p(p);
        let (node,slot) = self.cursor.remove(phys);
        return (node,slot);
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

    ///preorder block split (see trait doc). positions at the root, consumes self.
    ///splits the root inode at its median child; split-and-rotates the block at the
    ///boundary (child[mid]'s phys — pre-order: a child precedes its subtree, so its own
    ///slot is the subtree's first = the slice point), so the right half's nodes land on
    ///odd slots leaving phys 0 free; places the new right-block root there (find_slot
    ///before the first occupied is a no-op slide) and repoints the right half's direct
    ///children — their `parent` still names the old root, now in the left half — at the
    ///new root. returns the right block + separator; the caller wires both block roots
    ///under an arena parent.
    fn split_tree(mut self) -> (B, <B::T as Node>::K) {
        let root_v = self.cursor.root_v();
        self.cursor.vseek(root_v);
        self.depth = 0;

        //boundary = phys of child[n/2] (pre-order: the child is its subtree's first node).
        let child_mid_v = {
            let root = self.cursor.current().expect("split_tree: no root");
            let n = unsafe { root.orphan.inode.children.len() };
            debug_assert!(n >= 2, "split_tree: root needs >=2 children");
            *unsafe { root.orphan.inode.children.get(n / 2) }
        };
        let at = self.cursor.v2p(child_mid_v);

        //split the root in place: self keeps the left children, blank = right half + sep.
        let mut blank = BNode {
            orphan: OrphanUnionNode { inode: BINode::default() },
            parent: P::MIN,
        };
        let sep = {
            let root = self.cursor.current_mut().expect("split_tree: no root");
            unsafe { root.orphan.inode.split_into(&mut blank.orphan.inode) }
        };

        //consume the walker → &mut block, split-and-rotate at the boundary. the right
        //half's nodes land on odd slots (spread offset 1) leaving phys 0 free. R_left
        //stays at its slot; split_block_and_rotate clobbered the root to P::MAX — restore.
        let cursor = self.current_into();
        let (block, _) = cursor.into_parts();
        let mut right = block.split_block_and_rotate(at);
        block.set_root(root_v);

        //place the new right root at phys 0 (the free slot the spread opened). find_slot
        //before the first occupied lands on it with from==to — a no-op slide.
        let (new_root_v, new_phys) = {
            let mut rc = BlockCursor::new(&mut right);
            let first = rc.position().expect("split_tree: right half empty");
            let found = rc.find_slot(first, false, None);
            let slot = match found.slide {
                Some(ns) => {
                    debug_assert_eq!(ns.from, ns.to, "split_tree: expected no-op slide (phys 0 free)");
                    rc.slide_none(ns, None)
                }
                None => panic!("split_tree: no front None after split-and-rotate"),
            };
            let phys = rc.insert(blank, slot);
            (rc.p2v(phys), phys)
        };
        right.set_root(new_root_v);

        //repoint the right half's direct children: their `parent` still names the old root
        //(now in the left half — a cross-block vaddr) → set it to the new right root.
        let child_vs: Vec<P> = {
            let r = right.get_mut(new_phys);
            let n = unsafe { r.orphan.inode.children.len() };
            let mut vs = Vec::with_capacity(n);
            for i in 0..n {
                vs.push(*unsafe { r.orphan.inode.children.get(i) });
            }
            vs
        };
        for cv in child_vs {
            right.vget_mut(cv).set_parent(new_root_v);
        }

        (right, sep)
    }
}

//Stage 1 split driver: leaf split + root promotion (height 0 -> 1). the cursor must
//be on the full leaf L (depth == height). `k`,`v` is the incoming pair that filled L.
impl<'block, 'walker, K, V, P, B>
    Probe<'block, B, BlockCursor<'block, 'walker, B, &'walker mut B>, Height>
where
    'block: 'walker,
    B: TreeBlockMut<'block, T = BNode<K, V, P>, Meta = Height, P = P, V = V>,
    K: C + Ord,
    V: C,
    P: BlockIndex,
    BNode<K, V, P>: HasParent<P>,
{
    ///placement core factored out of `insert_child` (and reused by the split paths):
    ///`find_slot` at `anchor_phys` (dir=after, pin=root) → run-parent-fixup →
    ///`slide_none` → re-resolve the parent vaddr (it may move) → `set_parent` →
    ///`insert`. returns `(new_v, parent_v_now)`. does NOT walk to the anchor and does
    ///NOT wire into the parent — the caller does both. the node's `parent` field is set
    ///to the (post-slide) parent vaddr.
    fn place_at(&mut self, anchor_phys: usize, parent_v: P, mut node: BNode<K, V, P>) -> (P, P) {
        let root_phys = self.cursor.root_phys();
        let found = self.cursor.find_slot(anchor_phys, true, Some(root_phys));
        let ns = found.slide.expect("place_at: block exhausted");
        let delta = ns.delta;
        let lo = ns.from.min(ns.to);
        let hi = ns.from.max(ns.to);
        //run-parent-fixup before the slide (position-neutral — saves/restores cursor).
        self.fixup(&ns);
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
        node.set_parent(parent_v_now);
        let new_phys = self.cursor.insert(node, opened);
        let new_v = self.cursor.p2v(new_phys);
        (new_v, parent_v_now)
    }

    ///dispatch: leaf-root -> split_root; leaf-with-parent -> split into the (roomy)
    ///parent. (Stage 1: the parent is never full — internal split is Stage 2.)
    fn split_leaf(&mut self, k: K, v: V) {
        if self.depth == 0 {
            self.split_root(k, v);
        } else {
            self.split_leaf_into_parent(k, v);
        }
    }

    ///L is a non-root leaf (cursor on L, depth == height), full. place an empty
    ///right-sibling leaf after L (`place_at`, no wiring), drain L into it and route the
    ///incoming (k,v), then PROPAGATE `(sep, new_r_v)` to L's parent: wire it in if the
    ///parent has room, else `split_internal` the (full) parent (which recurses up). L is
    ///the placement anchor and may move in `place_at`'s opposite-side slide, so its vaddr
    ///is re-derived by re-routing k from the root after the placement (new_r is placed-
    ///but-unwired/floating, so k's path still ends at L).
    fn split_leaf_into_parent(&mut self, k: K, v: V) {
        //sep = first key of the right half (B+ leaf split point).
        let sep = {
            let leaf = unsafe { &self.cursor.current().expect("split: no leaf").orphan.lnode };
            *leaf.keys.get(leaf.keys.len() / 2)
        };
        let leaf_phys = self.cursor.position().expect("split: no leaf");
        let parent_v = self.cursor.current().expect("split: no leaf").parent();
        //place an empty right-sibling leaf after L (anchor = L).
        let (new_r_v, _) = self.place_at(leaf_phys, parent_v, BNode::default());
        //L (anchor) may have moved; re-route k from the root to L's current vaddr.
        self.cursor.vseek(self.cursor.root_v());
        self.depth = 0;
        self.walk_to(&k);
        let leaf_v = self.cursor.address().expect("split: leaf gone");
        //drain L -> new_r and route (k,v). (k != sep: sep was an existing key, k is new.)
        {
            let (l_mut, nr_mut) = self.cursor.get_disjoint(leaf_v, new_r_v);
            let s = unsafe { l_mut.orphan.lnode.split_into(&mut nr_mut.orphan.lnode) };
            if k < s {
                unsafe { l_mut.orphan.lnode.insert(k, v) };
            } else {
                unsafe { nr_mut.orphan.lnode.insert(k, v) };
            }
        }
        //propagate (sep, new_r_v) to L's parent: wire if room, else split the parent.
        self.cursor.vseek(leaf_v);
        let parent_v = self.cursor.current().expect("split: no leaf").parent();
        self.cursor.vseek(parent_v);
        self.depth = self.meta.0 - 1;
        let full = node_full(
            self.cursor.current().expect("split: no parent"),
            self.depth,
            self.meta.0,
        );
        {
            let par = self.cursor.current().expect("split: no parent");
            eprintln!(
                "[leaf_propagate] parent_v={:?} depth={} full={} children={} keys={}",
                parent_v, self.depth, full,
                unsafe { par.orphan.inode.children.len() },
                unsafe { par.orphan.inode.keys.len() },
            );
        }
        if !full {
            let par = self.cursor.current_mut().expect("split: no parent");
            unsafe { par.orphan.inode.insert_child(sep, new_r_v) };
        } else {
            self.split_internal(parent_v, self.depth, sep, new_r_v);
        }
    }

    ///L is the root leaf (height 0, depth 0). promote a new empty inode root R above it,
    ///then split L into R. preorder wants R before L; since the root pin would forbid
    ///moving L out of phys 0, we place R *after* L (pin = L, roomy on that side) and swap
    ///the two slots so R lands at the root vaddr and L becomes its first child. an lnode
    ///never becomes an inode — a fresh inode is inserted above the leaf.
    fn split_root(&mut self, k: K, v: V) {
        let l_phys = self.cursor.position().expect("split_root: no root");
        debug_assert_eq!(self.cursor.root_phys(), l_phys, "split_root: cursor not on root");
        //place an empty inode R after L (find_slot grows + opens a slot on the after side;
        //pin = L keeps L put). block: [L(0)] -> [L(0), R(1)].
        let found = self.cursor.find_slot(l_phys, true, Some(l_phys));
        let opened = found
            .slide
            .map(|ns| self.cursor.slide_none(ns, Some(l_phys)))
            .expect("split_root: block exhausted placing R");
        let r = BNode { orphan: OrphanUnionNode { inode: BINode::default() }, parent: P::MIN };
        let r_phys = self.cursor.insert(r, opened);
        //swap R to the front: R takes the root vaddr (phys 0), L becomes phys 1.
        self.cursor.swap(l_phys, r_phys);
        let root_v = self.cursor.p2v(l_phys); //R is now at phys l_phys
        let l_v = self.cursor.p2v(r_phys);     //L is now at phys r_phys
        self.cursor.set_root(root_v);
        //wire the single edge R -> L (one child, no separator).
        let r_node = self.cursor.get_mut_phys(l_phys);
        unsafe { r_node.orphan.inode.children.push(l_v) };
        let l_node = self.cursor.get_mut_phys(r_phys);
        l_node.set_parent(root_v);
        self.cursor.set_meta(Height(self.meta.0 + 1));
        self.meta = Height(self.meta.0 + 1);
        //position on L (now a child at depth 1) and split it into R.
        self.cursor.vseek(l_v);
        self.depth = 1;
        self.split_leaf_into_parent(k, v);
    }

    ///split a full internal node Y (cursor on Y, `y_v`/`y_depth`) into Y(left) + a new
    ///right sibling `new_r`, ABSORBING the incoming `(sep_in, child_in)` into whichever
    ///half's range contains it. returns `(sep_Y, new_r_v)` for the caller to propagate;
    ///does NOT wire into Y's parent. the empty new_r is placed at the gap between Y's
    ///left and right subtrees (`target_gap = rightmost_desc(c[mid-1])+1`) — Y is before
    ///that gap, so Y does not move (y_v stays valid for the drain). the slide moves Y's
    ///right-half children; the run-parent-fixup rewrites Y.children (Y is still their
    ///parent, full+wired), so the drain copies correct post-slide child vaddrs into new_r.
    fn split_inode(&mut self, y_v_in: P, y_depth: u64, sep_in: K, child_in: P) -> (K, P, P) {
        let mid = {
            let y = self.cursor.current().expect("split_inode: no Y");
            unsafe { y.orphan.inode.children.len() / 2 }
        };
        //measure the anchor = rightmost desc of c[mid-1] (an occupied leaf — find_slot's
        //pos must be occupied; it opens the slot AFTER it, between Y's left and right
        //subtrees). then restore cursor on Y.
        let _ = self.descend(mid - 1);
        self.descend_right(self.meta.0 as usize - self.depth as usize);
        let anchor_phys = self.cursor.position().expect("split_inode: no anchor");
        self.cursor.vseek(y_v_in);
        self.depth = y_depth;
        let parent_v = self.cursor.current().expect("split_inode: no Y").parent();
        let new_r = BNode { orphan: OrphanUnionNode { inode: BINode::default() }, parent: P::MIN };
        let (new_r_v, _) = self.place_at(anchor_phys, parent_v, new_r);
        //Y may have moved in the placement slide (a before-side None can put Y in the
        //run), so the captured y_v can be stale. re-derive it: route sep_in (which is in
        //Y's range — the incoming child is added to Y) from the root down to y_depth.
        //new_r is placed-but-unwired (floating), so the path still ends in Y's subtree.
        self.cursor.vseek(self.cursor.root_v());
        self.depth = 0;
        self.walk_to(&sep_in);
        for _ in 0..(self.meta.0 - y_depth) {
            self.ascend();
        }
        let y_v = self.cursor.address().expect("split_inode: Y gone after place");
        eprintln!(
            "[split_inode] y_depth={} meta={} passed_yv={:?} rederived_yv={:?} new_r_v={:?}",
            y_depth, self.meta.0, y_v_in, y_v, new_r_v
        );
        //drain Y -> new_r, route incoming. record which half absorbed child_in.
        let routed_to_new_r;
        let sep_y = {
            let (y_mut, nr_mut) = self.cursor.get_disjoint(y_v, new_r_v);
            let s = unsafe { y_mut.orphan.inode.split_into(&mut nr_mut.orphan.inode) };
            routed_to_new_r = sep_in >= s;
            if routed_to_new_r {
                unsafe { nr_mut.orphan.inode.insert_child(sep_in, child_in) };
            } else {
                unsafe { y_mut.orphan.inode.insert_child(sep_in, child_in) };
            }
            s
        };
        //the incoming child_in's parent is stale (Y's pre-slide vaddr, or the old root
        //vaddr in the root-internal case) — it's now a child of Y or new_r, so repoint it
        //explicitly. (the new_r repoint below also covers a new_r-routed child_in.)
        {
            let p = if routed_to_new_r { new_r_v } else { y_v };
            self.cursor.vseek(child_in);
            self.cursor.current_mut().expect("split_inode: no child_in").set_parent(p);
        }
        //repoint the original right-half children (now new_r's) at new_r. their parent was
        //Y (== re-derived y_v — fixup kept it current through the slide); child_in was
        //already repointed above, so skip asserting its (stale) old parent.
        self.cursor.vseek(new_r_v);
        let n = unsafe { self.cursor.current().expect("split_inode: no new_r").orphan.inode.children.len() };
        for j in 0..n {
            let cv = *unsafe {
                self.cursor.current().expect("split_inode: no new_r").orphan.inode.children.get(j)
            };
            self.cursor.vseek(cv);
            if cv != child_in {
                debug_assert_eq!(
                    self.cursor.current().expect("split_inode: no child").parent(),
                    y_v,
                    "split_inode: cv not Y's child (stale?)"
                );
            }
            self.cursor.current_mut().expect("split_inode: no child").set_parent(new_r_v);
            self.cursor.vseek(new_r_v);
        }
        //Y's (hoisted) parent — fixup kept it current through the slide; return it so the
        //caller propagates without re-finding Y.
        self.cursor.vseek(y_v);
        let parent_v = self.cursor.current().expect("split_inode: no Y").parent();
        (sep_y, new_r_v, parent_v)
    }

    ///split a full internal node Y (cursor on Y, `y_v`/`y_depth`), absorbing incoming
    ///`(sep_in, child_in)`, and PROPAGATE `(sep_Y, new_r_v)` to Y's parent: wire it in if
    ///the parent has room, else recurse `split_internal` on the (full) parent. Y is the
    ///root -> `split_root_internal` (promote a new root) instead.
    fn split_internal(&mut self, y_v: P, y_depth: u64, sep_in: K, child_in: P) {
        if y_depth == 0 {
            self.split_root_internal(sep_in, child_in);
            return;
        }
        let (sep_y, new_r_v, parent_v) = self.split_inode(y_v, y_depth, sep_in, child_in);
        //split_inode returned Y's current parent (Y may have moved in its placement
        //slide; fixup kept the child→parent field current). propagate (sep_y, new_r_v).
        self.cursor.vseek(parent_v);
        self.depth = y_depth - 1;
        let full = node_full(
            self.cursor.current().expect("split_internal: no parent"),
            y_depth - 1,
            self.meta.0,
        );
        if !full {
            let par = self.cursor.current_mut().expect("split_internal: no parent");
            unsafe { par.orphan.inode.insert_child(sep_y, new_r_v) };
        } else {
            self.split_internal(parent_v, y_depth - 1, sep_y, new_r_v);
        }
    }

    ///Y is the root inode (full, depth 0), absorbing incoming `(sep_in, child_in)`.
    ///promote a new empty inode root R' above Y (place after, swap to the front so R'
    ///takes the root vaddr), then split Y into R' (absorbing the incoming) and wire
    ///`(sep_Y, new_r)` into R'. height++.
    fn split_root_internal(&mut self, sep_in: K, child_in: P) {
        let y_phys = self.cursor.position().expect("split_root_internal: no root");
        debug_assert_eq!(self.cursor.root_phys(), y_phys, "split_root_internal: not on root");
        let found = self.cursor.find_slot(y_phys, true, Some(y_phys));
        let opened = found
            .slide
            .map(|ns| self.cursor.slide_none(ns, Some(y_phys)))
            .expect("split_root_internal: block exhausted placing R'");
        let rp = BNode { orphan: OrphanUnionNode { inode: BINode::default() }, parent: P::MIN };
        let rp_phys = self.cursor.insert(rp, opened);
        self.cursor.swap(y_phys, rp_phys);
        let root_v = self.cursor.p2v(y_phys); //R' now at phys y_phys
        let y_v = self.cursor.p2v(rp_phys);    //Y now at phys rp_phys
        self.cursor.set_root(root_v);
        //wire R' -> Y (one child, no separator), Y.parent = R'.
        let rp_node = self.cursor.get_mut_phys(y_phys);
        unsafe { rp_node.orphan.inode.children.push(y_v) };
        let y_node = self.cursor.get_mut_phys(rp_phys);
        y_node.set_parent(root_v);
        //Y moved in the swap (y_phys -> rp_phys), so Y's children's hoisted `parent`
        //field still points at Y's old vaddr (root_v, now R') — stale. repoint Y's direct
        //children at Y's new vaddr y_v. (grandchildren point at Y's children, which the
        //swap did not touch, so they stay correct.)
        self.cursor.vseek(y_v);
        let n = unsafe { self.cursor.current().expect("split_root_internal: no Y").orphan.inode.children.len() };
        for j in 0..n {
            let cv = *unsafe {
                self.cursor.current().expect("split_root_internal: no Y").orphan.inode.children.get(j)
            };
            self.cursor.vseek(cv);
            self.cursor.current_mut().expect("split_root_internal: no child").set_parent(y_v);
            self.cursor.vseek(y_v);
        }
        self.cursor.set_meta(Height(self.meta.0 + 1));
        self.meta = Height(self.meta.0 + 1);
        //position on Y (now a child at depth 1) and split it into R' (absorbing incoming).
        self.cursor.vseek(y_v);
        self.depth = 1;
        let (sep_y, new_r_v, _) = self.split_inode(y_v, 1, sep_in, child_in);
        //wire (sep_y, new_r_v) into R' (roomy: one child).
        self.cursor.vseek(root_v);
        self.depth = 0;
        let rp = self.cursor.current_mut().expect("split_root_internal: no R'");
        unsafe { rp.orphan.inode.insert_child(sep_y, new_r_v) };
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
        //root leaf inited here (height 0); `insert` then never checks for an empty
        //block. `remove` only edits leaf keys — it never frees the root block slot, so
        //`block.len()` stays ≥ 1 for the life of the map.
        let mut block = <MapBlock<K, V, P, CAP> as BlockMutTrait>::new();
        let phys = block.insert_root(BNode::default());
        block.set_root(block.p2v(phys));
        Self { block, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        let mut w = Probe::new(*self.block.meta(), &self.block);
        w.walk_to(k)?;
        let (block, phys) = w.current_into().into_parts();
        let leaf = unsafe { &block.get(phys?).orphan.lnode };
        let (i, cmp) = leaf.lookup(k);
        if cmp == Ordering::Equal { Some(leaf.values.get(i)) } else { None }
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let m = *self.block.meta();
        let mut w = Probe::new_mut(m, &mut self.block);
        w.walk_to(k)?;
        let (block, phys) = w.current_into().into_parts();
        let leaf = unsafe { &mut block.get_mut(phys?).orphan.lnode };
        let (i, cmp) = leaf.lookup(k);
        if cmp == Ordering::Equal { Some(leaf.values.get_mut(i)) } else { None }
    }

    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let height = *self.block.meta();
        let mut w = Probe::new_mut(height, &mut self.block);
        w.walk_to(&k)?;
        let (i, cmp, full) = {
            let leaf = unsafe { &w.current_mut().expect("insert: no leaf").orphan.lnode };
            let (i, cmp) = leaf.lookup(&k);
            (i, cmp, leaf.keys.len() >= LEAF_MAX)
        };
        if cmp == Ordering::Equal {
            let leaf = unsafe { &mut w.current_mut().expect("insert: no leaf").orphan.lnode };
            return Some(std::mem::replace(leaf.values.get_mut(i), v));
        }
        if full {
            w.split_leaf(k, v);
            self.len += 1;
            return None;
        }
        let leaf = unsafe { &mut w.current_mut().expect("insert: no leaf").orphan.lnode };
        leaf.insert_at(i, k, v);
        self.len += 1;
        None
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let m = *self.block.meta();
        let mut w = Probe::new_mut(m, &mut self.block);
        w.walk_to(k)?;
        let (block, phys) = w.current_into().into_parts();
        let leaf = unsafe { &mut block.get_mut(phys?).orphan.lnode };
        let (i, cmp) = leaf.lookup(k);
        if cmp == Ordering::Equal {
            let (_, val) = leaf.remove(i);
            self.len -= 1;
            Some(val)
        } else {
            None
        }
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