use crate::RelTo;
use crate::blocks::{BlockTrait, OpenSlot};
use std::marker::PhantomData;
use crate::index::BlockIndex;
use crate::metadata::Fixable;
use crate::store::NoneSlide;
use crate::treeblock::TreeBlock;
use crate::{InOrder, PostOrder, PreOrder};

//default shorthand for stored types, temporary till we care.
pub trait SS: 'static + Sized {}
impl<T> SS for T where T: 'static + Sized {}

pub trait Node {
    type K: SS;
    type V: SS;
    type P: BlockIndex;
    ///maximum children per node (in-order layout's parent gap scales with it)
    const DEGREE: usize;
}

pub trait SplittableNode: Node {
    ///drain the right half into a new node; self keeps the left; returns the right half.
    fn split(&mut self) -> Self;
}

// ---------------------------------------------------------------------------
// layer 1 — consumer-implemented node mask. the consumer's walker struct implements
// these; the crate never sees the node representation (union/enum/whatever).
// ---------------------------------------------------------------------------

///stackless positioned reader over a block's nodes. no ascend — trees without stored
///parent pointers can still implement this (lookup needs descent only).
pub trait NodeCursor<'block, 'walker, B>: Sized
where
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    ///positioned at the root.
    fn from_block(b: &'walker B) -> Self;
    fn block(&self) -> &'walker B;
    ///phys of the current node.
    fn position(&self) -> usize;
    fn is_leaf(&self) -> bool;
    fn current(&self) -> &'walker B::N;
    ///number of children of the current node.
    fn child_count(&self) -> usize;
    ///vaddr of child `idx`.
    fn child(&self, idx: usize) -> B::P;
    ///node-level routing: the child index a lookup of `k` descends through.
    fn lookup(&self, k: &<B::N as Node>::K) -> usize;
    fn descend(&mut self, child_idx: usize) -> &'walker B::N;

    ///full root→leaf descent; returns the terminal node (None if the block is empty).
    fn walk_to(&mut self, k: &<B::N as Node>::K) -> Option<&'walker B::N> {
        if self.block().occupied() == 0 {
            return None;
        }
        while !self.is_leaf() {
            let idx = self.lookup(k);
            self.descend(idx);
        }
        Some(self.current())
    }
}

///ascend-capable cursor — the consumer's stackful walker. `Fixable` is load-bearing:
///the tree ops below hand the walker every grow/slide fixup and it must correct its own
///tracked state (position + ancestry) to stay valid.
pub trait NodeWalker<'block, 'walker, B>: NodeCursor<'block, 'walker, B> + Fixable<B::P>
where
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    fn depth(&self) -> usize;
    fn ascend(&mut self) -> &'walker B::N;
    ///(parent phys, child idx we descended through). None at the root.
    fn parent(&self) -> Option<(usize, usize)>;
}

///consumer mut surface: node-level reads/writes masked behind the walker.
pub trait NodeWalkerMut<'block, 'walker, B>: NodeWalker<'block, 'walker, B>
where
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    ///what `insert_child` places in a node — node-shape-specific: B-tree `(K, V|P)`,
    ///B+ `Child(P)` at inodes / `Value(V)` at leaves, binary `()`.
    type Payload;
    ///the payload for a newly placed child node: `k` = bounding separator (if the shape
    ///has one), `ptr` = the child's vaddr.
    fn child_payload(&self, k: &<B::N as Node>::K, ptr: B::P) -> Self::Payload;

    fn from_block_mut(b: &'walker mut B) -> Self;
    fn block_mut(&mut self) -> &mut B;
    fn current_mut(&mut self) -> &mut B::N;
    ///current node has room for one more child/payload.
    fn has_space(&self) -> bool;
    ///set the child pointer `child_idx` of the ancestor `up` levels above the current
    ///node (0 = current) to `ptr`. ancestry-aware, position-stable — the fixup path
    ///rewrites a parent's entry while standing on the child (a walk back down would
    ///descend through the just-rewritten pointer, which pre-slide names the wrong slot).
    fn set_child(&mut self, up: usize, child_idx: usize, ptr: B::P);
    ///set current node's parent field. no-op for parent-free shapes.
    fn set_parent(&mut self, ptr: B::P);
    ///node-level wire: place `payload` at child slot `child_idx`. no block interaction.
    fn insert_child(&mut self, child_idx: usize, payload: Self::Payload);
    ///node-level unwire: remove child `child_idx` (+ its bounding separator, shape-specific).
    fn remove_child(&mut self, child_idx: usize) -> Self::Payload;
}

// ---------------------------------------------------------------------------
// layer 2 — ordered traversal. the per-ordering semantics live on `OrderOps`,
// implemented for the ordering marker types (`PreOrder`/`InOrder`/`PostOrder`) —
// distinct self types, so no coherence clash — and the single `TreeWalk` impl
// dispatches statically through `B::O`. a consumer with a custom ordering implements
// `OrderOps` for it and gets the whole layer.
// ---------------------------------------------------------------------------

///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o: PhantomData<O>,
}

impl<O, NW> TreeWalker<O, NW> {
    pub fn new(nw: NW) -> Self {
        Self { nw, _o: PhantomData }
    }
}

///ordered traversal in the block's layout ordering, over the wrapper. `boundary` walks
///to the anchor slot for a node placed adjacent to `child_idx`'s subtree (`after` = the
///position following it, i.e. a new child at `child_idx + 1`); returns the anchor (side
///+ phys) and the levels descended — the caller ascends back.
pub trait TreeWalk<'block, 'walker, NW, B>
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    fn next(&mut self) -> Option<&'walker B::N>;
    fn prev(&mut self) -> Option<&'walker B::N>;
    fn first(&mut self) -> Option<&'walker B::N>;
    fn last(&mut self) -> Option<&'walker B::N>;
    fn boundary(&mut self, child_idx: usize, after: bool) -> (RelTo<usize>, usize);
}

// ---- shared walk helpers (free fns over the consumer walker) ----

fn at_root<'block, 'walker, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    while nw.parent().is_some() {
        nw.ascend();
    }
}

fn leftmost_leaf<'block, 'walker, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(0);
        levels += 1;
    }
    levels
}

fn rightmost_leaf<'block, 'walker, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(nw.child_count() - 1);
        levels += 1;
    }
    levels
}

fn anchor_after<'block, 'walker, NW, B>(nw: &NW) -> (RelTo<usize>, usize)
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    (RelTo::After(nw.position()), 0)
}

impl<'block, 'walker, NW, B> TreeWalk<'block, 'walker, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block, O = PreOrder> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    ///child then siblings' subtrees; after a leaf, the nearest ancestor's next sibling.
    fn next(&mut self) -> Option<&'walker B::N> {
        if !self.nw.is_leaf() {
            self.nw.descend(0);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            if idx + 1 < self.nw.child_count() {
                self.nw.descend(idx + 1);
                return Some(self.nw.current());
            }
        }
    }

    ///parent for a first child; else the deepest-rightmost node of the prev sibling's subtree.
    fn prev(&mut self) -> Option<&'walker B::N> {
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            if idx > 0 {
                self.nw.descend(idx - 1);
                rightmost_leaf(&mut self.nw);
                return Some(self.nw.current());
            }
        }
    }

    fn first(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw); //preorder first = root
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        rightmost_leaf(&mut self.nw);
        Some(self.nw.current())
    }

    ///preorder: node precedes its subtree. new child 0 → after self; new child k →
    ///before old `child(k)` (its node is subtree-first); append → after the
    ///deepest-rightmost node.
    fn boundary(&mut self, child_idx: usize, after: bool) -> (RelTo<usize>, usize) {
        let cc = self.nw.child_count();
        if cc == 0 {
            return anchor_after(&self.nw); //childless: after the parent
        }
        debug_assert!(child_idx <= cc, "boundary: child_idx out of range");
        let k = child_idx + after as usize;
        if k == 0 {
            anchor_after(&self.nw)
        } else if k < cc {
            let p = self.nw.block().v2p(self.nw.child(k));
            (RelTo::Before(p), 0)
        } else {
            let levels = rightmost_leaf(&mut self.nw);
            (RelTo::After(self.nw.position()), levels)
        }
    }
}

impl<'block, 'walker, NW, B> TreeWalk<'block, 'walker, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block, O = InOrder> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    ///B-tree in-order: a node sits in the gap between `child[cc/2 - 1]` and `child[cc/2]`
    ///(mid = cc >> 1). successor of an internal node = leftmost leaf of `child[mid]`'s
    ///subtree; of a leaf = next sibling, the parent when crossing the gap.
    fn next(&mut self) -> Option<&'walker B::N> {
        if !self.nw.is_leaf() {
            self.nw.descend(self.nw.child_count() >> 1);
            leftmost_leaf(&mut self.nw);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            let cc = self.nw.child_count();
            if idx + 1 == cc >> 1 {
                return Some(self.nw.current()); //the parent follows child[mid-1]
            }
            if idx + 1 < cc {
                self.nw.descend(idx + 1);
                leftmost_leaf(&mut self.nw);
                return Some(self.nw.current());
            }
        }
    }

    ///mirror of `next`: predecessor of an internal node = rightmost leaf of
    ///`child[mid-1]`'s subtree; of a leaf = prev sibling, the parent before `child[mid]`.
    fn prev(&mut self) -> Option<&'walker B::N> {
        let cc = self.nw.child_count();
        if cc >= 2 {
            self.nw.descend((cc >> 1) - 1);
            rightmost_leaf(&mut self.nw);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            let cc = self.nw.child_count();
            if idx == cc >> 1 {
                return Some(self.nw.current()); //the parent precedes child[mid]
            }
            if idx > 0 {
                self.nw.descend(idx - 1);
                rightmost_leaf(&mut self.nw);
                return Some(self.nw.current());
            }
        }
    }

    fn first(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        leftmost_leaf(&mut self.nw);
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        rightmost_leaf(&mut self.nw);
        Some(self.nw.current())
    }

    ///in-order gap inserts land AFTER the parent (a new child at `mid` takes a slot past
    ///it; the parent hops later if it must). both gap-side queries (`before child[mid]`,
    ///`after child[mid-1]`) name the same gap → `After(self)`, a fast path over the
    ///general descend+boundary-walk.
    fn boundary(&mut self, child_idx: usize, after: bool) -> (RelTo<usize>, usize) {
        let cc = self.nw.child_count();
        if cc == 0 {
            return anchor_after(&self.nw); //childless: after the parent
        }
        debug_assert!(child_idx < cc, "boundary: child_idx out of range");
        let mid = cc >> 1;
        if !after && child_idx == mid || after && child_idx + 1 == mid {
            return anchor_after(&self.nw);
        }
        self.nw.descend(child_idx);
        if after {
            let levels = 1 + rightmost_leaf(&mut self.nw);
            (RelTo::After(self.nw.position()), levels)
        } else {
            let levels = 1 + leftmost_leaf(&mut self.nw);
            (RelTo::Before(self.nw.position()), levels)
        }
    }
}

impl<'block, 'walker, NW, B> TreeWalk<'block, 'walker, NW, B> for TreeWalker<PostOrder, NW>
where
    NW: NodeWalker<'block, 'walker, B>,
    B: BlockTrait<'block, O = PostOrder> + 'walker,
    B::N: Node,
    'block: 'walker,
{
    ///postorder: subtree then node. next = first (leftmost) node of the next sibling's
    ///subtree, the parent for a last child, None at the root (postorder last).
    fn next(&mut self) -> Option<&'walker B::N> {
        let (_, idx) = self.nw.parent()?;
        self.nw.ascend();
        if idx + 1 < self.nw.child_count() {
            self.nw.descend(idx + 1);
            leftmost_leaf(&mut self.nw);
        }
        Some(self.nw.current())
    }

    ///mirror: prev = own last child (a child node is its subtree's last), else the
    ///previous sibling node, walking up past first children.
    fn prev(&mut self) -> Option<&'walker B::N> {
        let cc = self.nw.child_count();
        if cc > 0 {
            self.nw.descend(cc - 1);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            if idx > 0 {
                self.nw.descend(idx - 1);
                return Some(self.nw.current());
            }
        }
    }

    fn first(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        leftmost_leaf(&mut self.nw); //postorder first = leftmost leaf
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&'walker B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw); //postorder last = root
        Some(self.nw.current())
    }

    ///postorder: node follows its subtree. new child 0 → before everything of child 0's
    ///subtree (leftmost leaf); new child k → after `child(k-1)` (its node is
    ///subtree-last, no descent).
    fn boundary(&mut self, child_idx: usize, after: bool) -> (RelTo<usize>, usize) {
        let cc = self.nw.child_count();
        if cc == 0 {
            return (RelTo::Before(self.nw.position()), 0); //childless: before the parent
        }
        debug_assert!(child_idx <= cc, "boundary: child_idx out of range");
        let k = child_idx + after as usize;
        if k == 0 {
            self.nw.descend(0);
            let levels = 1 + leftmost_leaf(&mut self.nw);
            (RelTo::Before(self.nw.position()), levels)
        } else {
            let p = self.nw.block().v2p(self.nw.child(k - 1));
            (RelTo::After(p), 0)
        }
    }
}

// ---------------------------------------------------------------------------
// layer 3 — tree ops. crate-implemented over the unified `BlockOps` surface; the
// per-ordering `boundary` comes in through the `TreeWalk` supertrait.
// ---------------------------------------------------------------------------

///`TreeWalkMut::insert_child` failure modes. splits are future work — the caller
///handles both by splitting the node/block and retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertErr {
    ///the current node has no room for another child (split the node).
    NodeFull,
    ///the block is exhausted — no slot, no spread, no edge room (split the block).
    BlockExhausted,
}

pub trait TreeWalkMut<'block, 'walker, NW, B>: TreeWalk<'block, 'walker, NW, B>
where
    NW: NodeWalkerMut<'block, 'walker, B>,
    B: TreeBlock<'block> + 'walker,
    B::N: Node + Default,
    'block: 'walker,
{
    ///place `node` as a new child of the current node, routed by `k` (the new child
    ///takes the slot `lookup(k)` returns): boundary walk to the anchor → `find_slot` →
    ///grow fixups → run-parent-fixup → `slide_none` → `insert` → ascend → node-level
    ///wire. the walker ends at the parent, post-everything.
    fn insert_child(&mut self, k: &<B::N as Node>::K, node: B::N) -> Result<B::P, InsertErr>;
    ///remove child `child_idx` of the current node: node-level unwire + block slot free.
    ///returns the removed node and its freed slot. no slide involved — no fixups.
    fn remove_child(&mut self, child_idx: usize) -> (B::N, OpenSlot);
    ///run-parent-fixup for a pending slide `ns` — BEFORE the slide is applied, rewrite
    ///each moved node's parent→child pointer (and the moved node's stored parent field
    ///when its parent also moved; no-op for parent-free shapes). the walker must be
    ///positioned at the slide's anchor with valid ancestry.
    fn fixup(&mut self, ns: &NoneSlide);
}

///generic over `O`: the supertrait obligation (`TreeWalker<O, NW>: TreeWalk`) is
///supplied as a where-clause rather than proven — it only discharges for a concrete
///`B`/`O` pair (one of the per-ordering `TreeWalk` impls), so the coverage is identical
///without needing per-ordering copies of the bodies.
impl<'block, 'walker, O, NW, B> TreeWalkMut<'block, 'walker, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, 'walker, B>,
    B: TreeBlock<'block> + 'walker,
    B::N: Node + Default,
    TreeWalker<O, NW>: TreeWalk<'block, 'walker, NW, B>,
    'block: 'walker,
{
    fn insert_child(&mut self, k: &<B::N as Node>::K, node: B::N) -> Result<B::P, InsertErr> {
        if !self.nw.has_space() {
            return Err(InsertErr::NodeFull);
        }
        let child_idx = self.nw.lookup(k);
        //anchor for the new child's slot (before old child[child_idx]'s subtree)
        let (rel, levels) = self.boundary(child_idx, false);
        let (anchor, after) = match rel {
            RelTo::Before(p) => (p, false),
            RelTo::After(p) => (p, true),
        };
        let found = self.nw.block_mut().find_slot(anchor, after);
        //the block may have grown (and fixed its own data) even on the exhaustion path —
        //the walker's state must follow either way.
        if let Some(g) = found.grew.as_ref() {
            let tr = self.nw.block().translator();
            self.nw.fixup(g, tr);
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        //fixup before the slide (vaddrs stable); then slide; then the walker follows.
        self.fixup(&ns);
        let open = self.nw.block_mut().slide_none(ns);
        let tr = self.nw.block().translator();
        self.nw.fixup(&ns, tr);
        //place + wire
        let phys = self.nw.block_mut().insert(node, open);
        let new_v = self.nw.block().p2v(phys);
        for _ in 0..levels {
            self.nw.ascend();
        }
        let payload = self.nw.child_payload(k, new_v);
        self.nw.insert_child(child_idx, payload);
        Ok(new_v)
    }

    fn remove_child(&mut self, child_idx: usize) -> (B::N, OpenSlot) {
        let phys = self.nw.block().v2p(self.nw.child(child_idx));
        let _separator = self.nw.remove_child(child_idx);
        self.nw.block_mut().remove(phys)
    }

    fn fixup(&mut self, ns: &NoneSlide) {
        if ns.from == ns.to {
            return;
        }
        let delta = ns.delta;
        let lo = ns.from.min(ns.to);
        let hi = ns.from.max(ns.to);
        //the moved run is the contiguous Some-run between `to` and `from` (the `from`
        //slot is the None); `steps` nodes in it. the walker sits at the slide's anchor:
        //delta>0 ⇒ items shift up ⇒ the run lies at/above the anchor ⇒ walk next();
        //delta<0 ⇒ below ⇒ prev(). the anchor may itself be the run's first/last moved
        //node. forward-only walking can't re-enter a processed node's subtree, so the
        //child entries it reads on the way are only ever unprocessed (correct) ones.
        let steps = hi - lo;
        let in_run = if delta > 0 { self.nw.position() == lo } else { self.nw.position() == hi };
        let mut walked = 0;
        if !in_run {
            let n = if delta > 0 { self.next() } else { self.prev() };
            debug_assert!(n.is_some(), "fixup: run walk fell off the block");
            walked = 1;
        }
        for i in 0..steps {
            let p = self.nw.position();
            if let Some((pphys, idx)) = self.nw.parent() {
                //parent moved iff its phys is inside the closed run (it can't be `from`
                //— that's the None slot).
                let parent_moved = pphys != ns.from && lo <= pphys && pphys <= hi;
                let pv = if parent_moved {
                    pphys.wrapping_add(delta as usize)
                } else {
                    pphys
                };
                //child→parent: repoint this node's stored parent field at the parent's
                //post-slide vaddr (no-op for parent-free shapes).
                self.nw.set_parent(self.nw.block().p2v(pv));
                //parent→child: rewrite the stale entry — `idx` from ancestry is
                //authoritative, no value scan, no descent.
                let new_v = self.nw.block().p2v(p.wrapping_add(delta as usize));
                self.nw.set_child(1, idx, new_v);
            }
            if i + 1 < steps {
                let n = if delta > 0 { self.next() } else { self.prev() };
                debug_assert!(n.is_some(), "fixup: run walk fell off the block");
                walked += 1;
            }
        }
        //position-neutral: walk back to the anchor so the caller's ascend count holds.
        for _ in 0..walked {
            let n = if delta > 0 { self.prev() } else { self.next() };
            debug_assert!(n.is_some(), "fixup: return walk fell off the block");
        }
    }
}

// ---------------------------------------------------------------------------
// layer 3 sketch — splits (clone-split driver, root promotion). declared, unwired.
// ---------------------------------------------------------------------------

pub trait SplitTreeWalker<'block, 'walker, NW, B>: TreeWalkMut<'block, 'walker, NW, B>
where
    NW: NodeWalkerMut<'block, 'walker, B>,
    B: TreeBlock<'block> + 'walker,
    B::N: SplittableNode + Default,
    'block: 'walker,
{
    ///split child `child_idx` into two nodes (bottom-up propagation per the split design).
    fn split_child(&mut self, child_idx: usize) -> Option<&mut B::N>;
}