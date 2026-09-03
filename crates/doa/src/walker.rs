use crate::blocks::{BlockOps, BlockTrait, OpenSlot};
use std::cmp::Ordering;
use std::marker::PhantomData;
use crate::index::BlockIndex;
use crate::metadata::Fixable;
use crate::store::NoneSlide;
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
///parent pointers can still implement this (lookup needs descent only). no constructor
///here: a mut-holding walker can't be built from a shared borrow, so construction lives
///on `From` bounds at the `TreeBlock` constructors.
pub trait NodeCursor<'block, B>: Sized
where
    B: BlockTrait<'block>,
    B::N: Node,
{
    fn block(&self) -> & B;
    ///phys of the current node.
    fn position(&self) -> usize;
    fn is_leaf(&self) -> bool;
    fn current(&self) -> & B::N;
    ///number of children of the current node.
    fn child_count(&self) -> usize;
    ///vaddr of child `idx`.
    fn child(&self, idx: usize) -> B::P;
    ///node-level relative position of `k` among the current node's ordered children:
    ///`(pos, cmp)`. `pos` = the child `search` descends to by default; `cmp` = `k`'s
    ///relation to that child — `Less` = before it (a new child takes slot `pos`),
    ///`Equal` = addressed by it / within its key span (slot `pos+1`), `Greater` = after
    /// it, before child `pos+1` (slot `pos+1`; `(len-1, Greater)` is the append case).
    ///routing is NOT decided here — `search` owns it, and the interpretation of the
    ///pair is the consumer's routing policy (a value-storing inode stops on `Equal`,
    ///B+ equal-right descends), so `search` has no default.
    fn lookup(&self, k: &<B::N as Node>::K) -> (usize, Ordering);
    fn descend(&mut self, child_idx: usize) -> &B::N;
    ///descend from the current node to `k`'s terminal (None conventionally = empty
    ///block). consumer-implemented routing over `lookup`'s `(pos, cmp)` — equal-right,
    ///Eq-stop, …: baking in a default would fix the meaning of `Equal`, which is a
    ///per-shape decision.
    fn search(&mut self, k: &<B::N as Node>::K) -> Option<&B::N>;
}

///ascend-capable cursor — the consumer's stackful walker.
pub trait NodeWalker<'block, B>: NodeCursor<'block, B>
where
    B: BlockTrait<'block>,
    B::N: Node,
{
    fn depth(&self) -> usize;
    fn ascend(&mut self) -> &B::N;
    ///(parent phys, child idx we descended through). None at the root.
    fn parent(&self) -> Option<(usize, usize)>;
}

///consumer mut surface: node-level reads/writes masked behind the walker.
pub trait NodeWalkerMut<'block, B>: NodeWalker<'block, B>
where
    B: BlockTrait<'block>,
    B::N: Node,
{
    ///what `insert_child` places in a node — node-shape-specific: B-tree `(K, V|P)`,
    ///B+ `Child(P)` at inodes / `Value(V)` at leaves, binary `()`.
    type Payload;
    ///the payload for a newly placed child node: `k` = bounding separator (if the shape
    ///has one), `ptr` = the child's vaddr.
    fn child_payload(&self, k: &<B::N as Node>::K, ptr: B::P) -> Self::Payload;

    ///the walker's fixable tracked state (position + ancestry). `Fixable` is
    ///load-bearing: the tree ops below hand it every grow/slide fixup and it must
    ///correct every address it holds. `Clone` snapshots it around `fixup`'s run walk.
    type State: Fixable<B::P> + Clone;
    ///split-borrow the walker: mutable state + shared block, from ONE call — two
    ///separate accessors would reintroduce the state-vs-block borrow conflict the
    /// fixup path hits (`state.fixup(f, block.translator())`).
    fn parts(&mut self) -> (&mut Self::State, &B);
    ///mutable-both split (the `set_child` path).
    fn parts_mut(&mut self) -> (&mut Self::State, &mut B);

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
// layer 2 — ordered traversal. the wrapper carries `O` as phantom data so the
// per-ordering `TreeWalk` impls sit on distinct self types (coherence); the wrapper's
// `O` is bound to the block's at every use (`B: BlockTrait<O = O>`).
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

///the insertion-anchor plan: the cheapest name for the gap a new child at slot
///`child_idx` occupies (between `child[idx-1]` and `child[idx]`). pure choice, no
///walking — `TreeWalkMut::insert_child` executes it.
pub enum Suggested {
    ///anchor = the current node (the parent); no walk.
    Parent { before: bool },
    ///anchor = child `idx`'s subtree edge — descend `idx`, then `subtree_first`
    ///(before) / `subtree_last` (after).
    Child { idx: usize, before: bool },
}

///ordered traversal in the block's layout ordering, over the wrapper. the per-ordering
///contributions: `next`/`prev`, the subtree edge walks, and the insertion-anchor
///suggestion; `first`/`last` are at_root + a subtree edge.
pub trait TreeWalk<'block, NW, B>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block>,
    B::N: Node,
{
    fn next(&mut self) -> Option<&B::N>;
    fn prev(&mut self) -> Option<&B::N>;
    fn first(&mut self) -> Option<&B::N>;
    fn last(&mut self) -> Option<&B::N>;
    ///walk to the first node of the current subtree; levels descended.
    fn subtree_first(&mut self) -> usize;
    ///walk to the last node of the current subtree; levels descended.
    fn subtree_last(&mut self) -> usize;
    ///cheapest anchor for a new child at slot `child_idx` (the gap).
    fn suggest_insertion(&self, child_idx: usize) -> Suggested;
}

// ---- shared walk helpers (free fns over the consumer walker) ----

fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block>,
    B::N: Node,
{
    while nw.parent().is_some() {
        nw.ascend();
    }
}

fn leftmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block>,
    B::N: Node,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(0);
        levels += 1;
    }
    levels
}

fn rightmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block>,
    B::N: Node,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(nw.child_count() - 1);
        levels += 1;
    }
    levels
}


impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PreOrder>,
    B::N: Node,
{
    ///child then siblings' subtrees; after a leaf, the nearest ancestor's next sibling.
    fn next(&mut self) -> Option<&B::N> {
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
    fn prev(&mut self) -> Option<&B::N> {
        let (_, idx) = self.nw.parent()?;
        self.nw.ascend();
        if idx > 0 {
            self.nw.descend(idx - 1);
            rightmost_leaf(&mut self.nw);
        }
        Some(self.nw.current())
    }

    fn first(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_first();
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_last();
        Some(self.nw.current())
    }

    ///the node precedes its subtree.
    fn subtree_first(&mut self) -> usize {
        0
    }
    fn subtree_last(&mut self) -> usize {
        rightmost_leaf(&mut self.nw)
    }

    ///gap 0 → after the parent (it precedes everything); mid gap → before `child(k)`
    ///(subtree-first — one descend); append → after the rightmost leaf.
    fn suggest_insertion(&self, child_idx: usize) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 || child_idx == 0 {
            return Suggested::Parent { before: false };
        }
        if child_idx < cc {
            Suggested::Child { idx: child_idx, before: true }
        } else {
            Suggested::Child { idx: cc - 1, before: false }
        }
    }
}

impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = InOrder>,
    B::N: Node,
{
    ///B-tree in-order: a node sits in the gap between `child[cc/2 - 1]` and `child[cc/2]`
    ///(mid = cc >> 1). successor of an internal node = leftmost leaf of `child[mid]`'s
    ///subtree; of a leaf = next sibling, the parent when crossing the gap.
    fn next(&mut self) -> Option<&B::N> {
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
    fn prev(&mut self) -> Option<&B::N> {
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

    fn first(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_first();
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_last();
        Some(self.nw.current())
    }

    ///the node sits mid-subtree; its edges are the outermost leaves.
    fn subtree_first(&mut self) -> usize {
        leftmost_leaf(&mut self.nw)
    }
    fn subtree_last(&mut self) -> usize {
        rightmost_leaf(&mut self.nw)
    }

    ///the node sits in the mid gap — a gap AT `mid` is the parent's own, so anchor
    ///here (no walk). other gaps → the adjacent child's edge leaf.
    fn suggest_insertion(&self, child_idx: usize) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { before: false };
        }
        debug_assert!(child_idx <= cc, "suggest_insertion: child_idx out of range");
        if child_idx == cc >> 1 {
            Suggested::Parent { before: false }
        } else if child_idx < cc {
            Suggested::Child { idx: child_idx, before: true }
        } else {
            Suggested::Child { idx: cc - 1, before: false }
        }
    }
}

impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PostOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PostOrder>,
    B::N: Node,
{
    ///postorder: subtree then node. next = first (leftmost) node of the next sibling's
    ///subtree, the parent for a last child, None at the root (postorder last).
    fn next(&mut self) -> Option<&B::N> {
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
    fn prev(&mut self) -> Option<&B::N> {
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

    fn first(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_first();
        Some(self.nw.current())
    }

    fn last(&mut self) -> Option<&B::N> {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_last();
        Some(self.nw.current())
    }

    ///the node follows its subtree.
    fn subtree_first(&mut self) -> usize {
        leftmost_leaf(&mut self.nw)
    }
    fn subtree_last(&mut self) -> usize {
        0
    }

    ///childless → before the parent (it follows everything); gap 0 → before child 0's
    ///subtree (leftmost leaf); gap k → after `child(k-1)` (subtree-last — one descend).
    fn suggest_insertion(&self, child_idx: usize) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { before: true };
        }
        debug_assert!(child_idx <= cc, "suggest_insertion: child_idx out of range");
        if child_idx == 0 {
            Suggested::Child { idx: 0, before: true }
        } else {
            Suggested::Child { idx: child_idx - 1, before: false }
        }
    }
}

// ---------------------------------------------------------------------------
// layer 3 — tree ops. crate-implemented over the unified `BlockOps` surface; the
// per-ordering `suggest_insertion`/subtree edges come in via the `TreeWalk` supertrait.
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

pub trait TreeWalkMut<'block, NW, B>: TreeWalk<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + BlockOps<'block>,
    B::N: Node + Default,
{
    ///place `node` as a new child of the current node, routed by `k` (the slot comes
    ///from `lookup`: `Less` ⇒ slot `pos`, `Equal`/`Greater` ⇒ slot `pos+1`; the caller
    ///guarantees the current node is a child-accepting internal — the consumer's
    ///`search` stops at terminals): `suggest_insertion` → walk to the anchor →
    ///`find_slot` → grow fixups → run-parent-fixup → `slide_none` → `insert` → ascend
    ///→ node-level wire. the walker ends at the parent, post-everything.
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
impl<'block, O, NW, B> TreeWalkMut<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + BlockOps<'block>,
    B::N: Node + Default,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn insert_child(&mut self, k: &<B::N as Node>::K, node: B::N) -> Result<B::P, InsertErr> {
        if !self.nw.has_space() {
            return Err(InsertErr::NodeFull);
        }
        //the slot the new child takes: Less ⇒ before child `pos`; Equal (k addressed
        //by it) / Greater ⇒ after it
        let (pos, cmp) = self.nw.lookup(k);
        let child_idx = pos + usize::from(cmp != Ordering::Less);
        //execute the suggestion: walk to the anchor, remember the descent depth
        let (anchor, after, levels) = match self.suggest_insertion(child_idx) {
            Suggested::Parent { before } => (self.nw.position(), !before, 0),
            Suggested::Child { idx, before } => {
                self.nw.descend(idx);
                let l = if before { self.subtree_first() } else { self.subtree_last() };
                (self.nw.position(), !before, 1 + l)
            }
        };
        let found = self.nw.block_mut().find_slot(anchor, after);
        //the block may have grown (and fixed its own data) even on the exhaustion path —
        //the walker's state must follow either way.
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            let tr = block.translator();
            state.fixup(g, tr);
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        //fixup before the slide (vaddrs stable); then slide; then the walker follows.
        self.fixup(&ns);
        let open = self.nw.block_mut().slide_none(ns);
        let (state, block) = self.nw.parts();
        let tr = block.translator();
        state.fixup(&ns, tr);
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
        //snapshot at the anchor, restored after — a walk back would descend through
        //just-rewritten (post-slide) child pointers over the still-pre-slide layout.
        let snapshot = self.nw.parts().0.clone();
        let in_run = if delta > 0 { self.nw.position() == lo } else { self.nw.position() == hi };
        if !in_run {
            let n = if delta > 0 { self.next() } else { self.prev() };
            debug_assert!(n.is_some(), "fixup: run walk fell off the block");
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
            }
        }
        //position-neutral: back at the anchor with entry ancestry — no walking.
        *self.nw.parts().0 = snapshot;
    }
}

// ---------------------------------------------------------------------------
// layer 3 sketch — splits (clone-split driver, root promotion). declared, unwired.
// ---------------------------------------------------------------------------

pub trait SplitTreeWalker<'block, NW, B>: TreeWalkMut<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + BlockOps<'block>,
    B::N: SplittableNode + Default,
{
    ///split child `child_idx` into two nodes (bottom-up propagation per the split design).
    fn split_child(&mut self, child_idx: usize) -> Option<&mut B::N>;
}