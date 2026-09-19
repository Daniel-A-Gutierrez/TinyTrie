//! the node contract (`Node`) + the walker layers. layer 1 — `NodeCursor`/
//! `NodeWalker`/`NodeWalkerMut`: the consumer-implemented mask over the node
//! representation; the walker IS its own state (`Fixable`), and the crate
//! reads `position` and moves the walker only through `descend`/`ascend` and
//! the internal state ops. layer 2 — `TreeWalker<O, NW>` + `TreeWalk`: ordered
//! traversal, one impl per ordering (pre, in; post unimplemented).
//! layer 3 — `PreOrderWalk`/`InOrderWalk`: the open surface. slot-moving ops
//! CONSUME the walker — a stale position is unrepresentable at the mutation
//! point; rotations keep it (nothing moves physically). wiring is
//! consumer-side: opens return slots + the applied fixups, the consumer
//! inserts and rewires via the block directly. `B` is a trait param at every
//! level; `O` is always `B::O` (the wrapper carries it as phantom data).
//! NOTE: traversal assumes packed ChildPos (rank == slot); sparse-addressed
//! nodes need children()-based sibling walks, unimplemented.

use crate::blocks::{BlockOps, BlockTrait, OpenSlot};
use crate::index::Addr;
use crate::metadata::{ChildPos, DoubleSlide, Fixable, GrewFixup, HasRoot, Pos};
use crate::store::NoneSlide;
use crate::{InOrder, PreOrder, Rel};
use std::marker::PhantomData;

///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}

pub type PreOrderWalker<NW> = TreeWalker<PreOrder, NW>;
pub type InOrderWalker<NW> = TreeWalker<InOrder, NW>;

///the block is exhausted — no slot, no spread, no edge room. split the block.
///the walker was consumed: rebuild from the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockExhausted;

///what an open applied: the grow remap + the slide pair (`b` is a no-op slide
/// for single opens). consumers holding addresses/positions across the open
/// apply these via their `Fixable` impl — held addrs need only the slides
/// (addrs are grow-stable by construction), held positions need both.
#[derive(Clone, Copy)]
pub struct OpenFixups {
    pub grew:   Option<GrewFixup>,
    pub slides: DoubleSlide,
}

pub trait Node {
    type K;
    type V;
    type A: Addr;
    ///maximum children per node (in-order layout's parent gap scales with it).
    const DEGREE: usize;
    ///do nodes store parent-pointer fields? gates the reparent machinery
    /// (`reparent_children`); false shapes pay nothing (the const check folds
    /// away).
    const STORES_PARENTS: bool;
}

// ---------------------------------------------------------------------------
// layer 1 — consumer-implemented node mask. the consumer's walker struct implements
// these; the crate never sees the node representation (union/enum/whatever).
// ---------------------------------------------------------------------------

///stackless positioned reader over a block's nodes. no ascend — trees without stored
///parent pointers can still implement this (lookup needs descent only). no constructor
///here: a mut-holding walker can't be built from a shared borrow, so construction lives
///on `From` bounds at the crate's free fns (`walker`/`search`).
pub trait NodeCursor<'block, B>: Sized
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    fn block(&self) -> &B;
    ///position of the current node. the crate's window for anchor computation.
    fn position(&self) -> Pos;
    fn is_root(&self) -> bool;
    fn is_leaf(&self) -> bool;
    ///number of children of the current node.
    fn child_count(&self) -> usize;
    ///addr of child `child`. contract: the crate's generic code gates every child
    ///access/descent on `is_leaf()` first; this (and the mut-layer child ops)
    ///PANIC when the current node can't support the operation.
    fn child(&self, child: ChildPos) -> B::A;
    ///the current node's child slots, in key order (slot ids may be sparse; they
    ///must sort like the keys). exact-size + double-ended: subtree edges and
    ///anchor scans run off it.
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, B::A)>
                             + ExactSizeIterator
                             + '_;
    ///node routing: the child to descend into for `k`, `None` = descent terminates
    ///at the current node. the consumer's equal/partial-match policy lives here.
    fn lookup(&self, k: &<B::N as Node>::K) -> Option<ChildPos>;
    ///descend into child `child`. consumer-impl'd: position update + descent
    /// record are the walker's own. the walker names the child afterward.
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b B::N
    where 'block: 'b;
    fn current<'b>(&'b self) -> &'b B::N
    where 'block: 'b {
        self.block().get(self.position())
    }
    ///descend by `k` until `lookup` returns None. the terminal node.
    fn search<'b>(&'b mut self, k: &<B::N as Node>::K) -> Option<&'b B::N>
    where 'block: 'b {
        while let Some(child) = self.lookup(k) {
            self.descend(child);
        }
        Some(self.current())
    }
}

///ascend-capable cursor — the consumer's stackful walker.
pub trait NodeWalker<'block, B>: NodeCursor<'block, B>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    ///move to the parent node. at the root this panics.
    ///(parent node, child slot we ascended through). the walker names the parent.
    fn ascend<'b>(&'b mut self) -> (&'b B::N, ChildPos)
    where 'block: 'b;
    ///(parent position, child slot we descended through); `None` at the root.
    fn parent(&self) -> Option<(Pos, ChildPos)>;
}

///consumer mut surface: `Fixable` so the crate's choreography corrects the walker's
///held state directly, plus the pointer-write primitives fixup delivery needs.
pub trait NodeWalkerMut<'block, B>: NodeWalker<'block, B> + Fixable<B::A>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    ///state snapshot for the crate's internal choreography (mid-op position
    /// restore). internal machinery — consumers implement, never call.
    type Snapshot;
    ///snapshot the walker's state.
    fn save(&self) -> Self::Snapshot;
    ///restore a snapshot. precondition: the block has not mutated since `save`.
    fn load(&mut self, snap: Self::Snapshot);
    ///reposition to `pos` — no tree meaning, state only. the crate's fixup
    /// machinery reaches moved members positionally; consumers never call.
    fn set_position(&mut self, pos: Pos);

    fn block_mut(&mut self) -> &mut B;
    ///set the child at `child` (levels `up`) to `addr`.
    fn set_child(&mut self, up: usize, child: ChildPos, addr: B::A);
    ///clear the child slot `child` (levels `up`) — no addr names it anymore.
    fn clear_child(&mut self, up: usize, child: ChildPos);
    ///set the current node's stored parent field. no-op for parent-free shapes.
    fn set_parent(&mut self, addr: B::A);
}

// ---------------------------------------------------------------------------
// layer 2 — ordered traversal.
// ---------------------------------------------------------------------------

///ordered traversal in the block's layout ordering, over the wrapper.
///`next`/`prev`, the subtree edge walks, `first`/`last`.
pub trait TreeWalk<'block, NW, B>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    fn next<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b;
    fn prev<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b;
    fn first<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b;
    fn last<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b;
    ///walk to the first node of the current subtree; levels descended.
    fn subtree_first(&mut self) -> usize;
    ///walk to the last node of the current subtree; levels descended.
    fn subtree_last(&mut self) -> usize;
}

// ---------------------------------------------------------------------------
// layer 3 — the open surface. consumes the walker; returns slots + fixups.
// ---------------------------------------------------------------------------

///preorder opens — B/B+ consumers.
pub trait PreOrderWalk<'block, NW, B>: TreeWalk<'block, NW, B> + Sized
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
{
    ///open a slot at the current node's own position, on the `rel` side.
    fn open_here(self, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    ///open a slot at child `child`'s subtree edge (Before/After the whole
    ///subtree), on the `rel` side.
    fn open_child(self, child: ChildPos, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    ///two independently-anchored opens, one atomic find pass — both slides
    /// computed before either moves.
    fn open_2_child(
        self,
        ca: ChildPos,
        ra: Rel,
        cb: ChildPos,
        rb: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///the slot a new parent of the current node takes (= before it).
    fn open_parent(self) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    ///root split: the new parent's slot + one child-edge slot, atomically.
    fn open_parent_child(
        self,
        child: ChildPos,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
}

///in-order opens + rotations — binary consumers. rotates move nothing
/// physically (rotation preserves the in-order sequence), so the walker
/// survives them.
pub trait InOrderWalk<'block, NW, B>: TreeWalk<'block, NW, B> + Sized
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: crate::metadata::HasRoot<B::A>,
{
    fn open_here(self, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    fn open_child(self, child: ChildPos, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    fn open_2_child(
        self,
        ca: ChildPos,
        ra: Rel,
        cb: ChildPos,
        rb: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///child 0 rises, current demotes to its slot 1, child 0's right subtree
    /// moves under the current's slot 0. ends on the riser; at the root the
    /// block root follows.
    fn rotate_right(&mut self);
    ///mirror: child 1 rises, current demotes to its slot 0, child 1's left
    /// subtree moves under the current's slot 1. ends on the riser.
    fn rotate_left(&mut self);
}

///(internal) where an open anchors: the current node's own position, or a
/// child's subtree edge.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    Here { rel: Rel },
    Child { idx: ChildPos, rel: Rel },
}

///layer 3 — crate-internal choreography: the slide engine + the open engines
/// over the unified `BlockOps` surface. machinery the consumer never calls —
/// only the crate's open impls do (pub in-module, not consumer surface). the
/// per-ordering nav comes in via the `TreeWalk` supertrait.
pub(crate) trait TreeWalkHelper<'block, NW, B>: TreeWalk<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
{
    fn reparent_children(&mut self, new_a: B::A);
    fn reparent_run(&mut self, ns: &NoneSlide);
    fn fixup(&mut self, ns: &NoneSlide);
    fn apply_slide(&mut self, ns: &NoneSlide) -> OpenSlot;
    fn walk_to_anchor(&mut self, anchor: Anchor) -> (Pos, Rel, usize);
    fn back_from_anchor(&mut self, levels: usize);
    fn open_at(&mut self, anchor: Anchor) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    fn open_2_at(
        &mut self,
        a: Anchor,
        b: Anchor,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
}

impl<O, NW> TreeWalker<O, NW> {
    pub fn new(nw: NW) -> Self {
        Self { nw, _o: PhantomData }
    }
}

impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block,
    B::N: Node,
{
    ///child then siblings' subtrees; after a leaf, the nearest ancestor's next sibling.
    fn next<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        if !self.nw.is_leaf() {
            self.nw.descend(ChildPos(0)); //walker: -> child 0
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend(); //walker: -> the parent
            if idx + 1 < self.nw.child_count() {
                self.nw.descend(idx + 1); //walker: -> the next sibling
                return Some(self.nw.current());
            }
        }
    }

    ///parent for a first child; else the deepest-rightmost node of the prev sibling's subtree.
    fn prev<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let (_, idx) = self.nw.parent()?;
        self.nw.ascend(); //walker: -> the parent
        if idx > 0 {
            self.nw.descend(idx - 1); //walker: -> the prev sibling
            rightmost_leaf(&mut self.nw);
        }
        Some(self.nw.current())
    }

    fn first<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_first();
        Some(self.nw.current())
    }

    fn last<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
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
}

impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block,
    B::N: Node,
{
    ///B-tree in-order: a node sits between `child[b-1]` and `child[b]`, `b =
    ///min(cc, DEGREE/2)` (`in_boundary`; after all children when b == cc). successor
    ///of an internal node = leftmost leaf of `child[b]`'s subtree (none when b == cc
    ///— the node ends its region); else next sibling, the parent when crossing b.
    fn next<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let cc = self.nw.child_count();
        let b = in_boundary::<B>(cc);
        if !self.nw.is_leaf() && b < cc {
            self.nw.descend(b); //walker: -> child[b]
            leftmost_leaf(&mut self.nw);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend(); //walker: -> the parent
            let cc = self.nw.child_count();
            let b = in_boundary::<B>(cc);
            if idx + 1 == b {
                return Some(self.nw.current()); //the parent follows child[b-1]
            }
            if idx + 1 < cc {
                self.nw.descend(idx + 1); //walker: -> the next sibling
                leftmost_leaf(&mut self.nw);
                return Some(self.nw.current());
            }
        }
    }

    ///mirror of `next`: predecessor of an internal node = `subtree_last` of
    ///`child[b-1]` (NOT bare `rightmost_leaf` — an after-all internal (b == cc)
    /// is its region's LAST node, so the descent must stop on it); of a leaf =
    /// prev sibling, the parent before `child[b]`.
    fn prev<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let cc = self.nw.child_count();
        let b = in_boundary::<B>(cc);
        if cc > 0 {
            self.nw.descend(b - 1); //walker: -> child[b-1]
            let _ = self.subtree_last();
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend(); //walker: -> the parent
            let cc = self.nw.child_count();
            let b = in_boundary::<B>(cc);
            if idx == b {
                return Some(self.nw.current()); //the parent precedes child[b]
            }
            if idx > 0 {
                self.nw.descend(idx - 1); //walker: -> the prev sibling
                let _ = self.subtree_last();
                return Some(self.nw.current());
            }
        }
    }

    fn first<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        if self.nw.block().occupied() == 0 {
            return None;
        }
        at_root(&mut self.nw);
        self.subtree_first();
        Some(self.nw.current())
    }

    fn last<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
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
    ///the region's last node: descend right while the node sits before its tail
    ///(b < cc); an after-all node (b == cc) IS the last — stop on it.
    fn subtree_last(&mut self) -> usize {
        let mut levels = 0;
        while !self.nw.is_leaf() {
            let cc = self.nw.child_count();
            if in_boundary::<B>(cc) == cc {
                return levels;
            }
            self.nw.descend(ChildPos(cc - 1)); //walker: -> child[cc-1]
            levels += 1;
        }
        levels
    }
}

///generic over `O`: the internal choreography. the `TreeWalker<O, NW>: TreeWalk`
///obligation is supplied as a where-clause rather than proven — it only discharges
///for a concrete `B`/`O` pair, so the coverage is identical without per-ordering
///copies of the bodies.
impl<'block, O, NW, B> TreeWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    ///point the current node's children's stored parent fields at `new_a`.
    ///`STORES_PARENTS`-gated: false shapes return immediately. only sound when
    ///the current node's child entries are consistent with the layout.
    fn reparent_children(&mut self, new_a: B::A) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        //collect first: `children()` holds the borrow the walks need
        let idxs: Vec<ChildPos> = self.nw.children().map(|(i, _)| i).collect();
        for idx in idxs {
            self.nw.descend(idx); //walker: -> child idx
            self.nw.set_parent(new_a);
            self.nw.ascend(); //walker: back
        }
    }

    ///the slide-companion to `reparent_children`: after `ns` is APPLIED, point each
    ///moved node's children's parent fields at the node's post-slide addr. must run
    ///post-slide — mid-fixup it would descend through just-rewritten (post-slide)
    ///entries over the still-pre-slide layout (subtle_bugs.md §3); post-slide every
    ///entry is consistent. position-based over the shifted run — no tree walk.
    ///position-restoring.
    fn reparent_run(&mut self, ns: &NoneSlide) {
        if !<B::N as Node>::STORES_PARENTS || ns.from == ns.to {
            return;
        }
        let (lo, hi) = (ns.from.min(ns.to), ns.from.max(ns.to));
        //post-slide member range: delta>0 ⇒ (lo, hi]; delta<0 ⇒ [lo, hi-1]
        let members = if ns.delta > 0 {
            (lo.0 + 1..=hi.0).collect::<Vec<_>>()
        } else {
            (lo.0..hi.0).collect::<Vec<_>>()
        };
        let back = self.nw.position();
        for q in members {
            self.nw.set_position(Pos(q)); //walker: -> the moved member (no tree meaning)
            let a = self.nw.block().p2a(Pos(q));
            self.reparent_children(a);
        }
        self.nw.set_position(back); //walker: restored
    }

    ///run-parent-fixup for a pending slide `ns` — BEFORE the slide is applied, rewrite
    ///each moved node's parent→child pointer (and the moved node's stored parent field
    ///when its parent also moved; no-op for parent-free shapes). the walker must be
    ///positioned at the slide's anchor with valid ancestry.
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
        let steps = hi.0 - lo.0;
        //snapshot at the anchor, restored after — a walk back would descend through
        //just-rewritten (post-slide) child pointers over the still-pre-slide layout.
        let snapshot = self.nw.save();
        let in_run =
            if delta > 0 { self.nw.position() == lo } else { self.nw.position() == hi };
        if !in_run {
            let n = if delta > 0 { self.next() } else { self.prev() };
            debug_assert!(n.is_some(), "fixup: run walk fell off the block");
        }
        for i in 0..steps {
            let p = self.nw.position();
            //per-visit canary: the run [lo, hi) is None-free by find_slot's
            //construction, so `steps` logical-order visits that all land inside
            //it are exactly its members. an inserted-but-unwired Some
            //(insert without wire) displaces a visit outside the range — the
            //tripwire for a ghost node breaking the layout (subtle_bugs.md §6).
            assert!(
                lo <= p && p <= hi,
                "fixup: run walk left the run — an occupied slot in the run is \
                 unwired (insert without wire?)"
            );
            if let Some((ppos, idx)) = self.nw.parent() {
                //parent moved iff its position is inside the closed run (it can't be
                //`from` — that's the None slot).
                let parent_moved = ppos != ns.from && lo <= ppos && ppos <= hi;
                let pa = if parent_moved { ppos.wrapping_add(delta as usize) } else { ppos };
                //child→parent: repoint this node's stored parent field at the parent's
                //post-slide addr (no-op for parent-free shapes).
                self.nw.set_parent(self.nw.block().p2a(pa));
                //parent→child: rewrite the stale entry — `idx` from ancestry is
                //authoritative, no value scan, no descent.
                let new_a = self.nw.block().p2a(p.wrapping_add(delta as usize));
                self.nw.set_child(1, idx, new_a);
            }
            if i + 1 < steps {
                let n = if delta > 0 { self.next() } else { self.prev() };
                debug_assert!(n.is_some(), "fixup: run walk fell off the block");
            }
        }
        //endpoint canary: against a consistent layout the walk visits the run
        //in slot order, so `steps` walk steps must end on the far edge exactly —
        //a ghost lands short/long.
        let far = if delta > 0 { hi - 1 } else { lo + 1 };
        assert_eq!(
            self.nw.position(),
            far,
            "fixup: run walk endpoint diverged — an occupied slot in the run is \
             unwired (insert without wire?), or the walk was entered mid-run"
        );
        //position-neutral: back at the anchor with entry ancestry — no walking.
        self.nw.load(snapshot);
    }

    ///apply a pending slide: run-parent-fixup → `slide_none` → walker-state fixup →
    ///`reparent_run` (STORES_PARENTS). THE chokepoint — every slide in the opens
    ///goes through here. returns the opened slot.
    fn apply_slide(&mut self, ns: &NoneSlide) -> OpenSlot {
        self.fixup(ns);
        let open = self.nw.block_mut().slide_none(*ns);
        let tr = self.nw.block().translator().clone();
        self.nw.slide_fix(*ns, &tr);
        self.reparent_run(ns);
        open
    }

    ///walk to `anchor`: (anchor position, open side, levels back to the current
    ///node). the walker is left AT the anchor — pair with `back_from_anchor`.
    fn walk_to_anchor(&mut self, anchor: Anchor) -> (Pos, Rel, usize) {
        match anchor {
            Anchor::Child { idx, rel } => {
                self.nw.descend(idx); //walker: -> the anchor subtree's root
                let e =
                    if rel == Rel::Before { self.subtree_first() } else { self.subtree_last() };
                (self.nw.position(), rel, 1 + e)
            }
            Anchor::Here { rel } => (self.nw.position(), rel, 0),
        }
    }

    ///ascend `levels` — the inverse of `walk_to_anchor`.
    fn back_from_anchor(&mut self, levels: usize) {
        for _ in 0..levels {
            self.nw.ascend(); //walker: one level toward the current node
        }
    }

    ///open one slot at `anchor` (find_slot + grow fixups + apply_slide). the
    ///walker's end position is meaningless to the caller — consumed either way.
    fn open_at(&mut self, anchor: Anchor) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        let (pos, rel, _) = self.walk_to_anchor(anchor);
        let found = self.nw.block_mut().find_slot(pos, rel);
        let Some(ns) = found.slide else {
            return Err(BlockExhausted);
        };
        if let Some(g) = found.grew {
            let tr = self.nw.block().translator().clone();
            self.nw.grew_fix(g, &tr);
        }
        let open = self.apply_slide(&ns);
        let noop = NoneSlide { from: open.0, to: open.0, delta: 0 };
        Ok((open, OpenFixups { grew: found.grew, slides: DoubleSlide { a: ns, b: noop } }))
    }

    ///two independent opens at `a`/`b` (find_2_slots, composed as one `DoubleSlide`
    ///fixup): both anchors are walked and both slides computed pre-mutation
    ///(both-slides-before-either, subtle_bugs.md §2), then applied one at a time —
    ///disjointness keeps each anchor valid across the other's slide, and the
    ///run-parent walks interleave with the slides (they cannot compose: a B-run
    ///member's parent may live in A's run, so walk B must see post-slide-A positions).
    fn open_2_at(
        &mut self,
        a: Anchor,
        b: Anchor,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        //both anchors walked (and returned from) before anything mutates
        let (pa, ra, la) = self.walk_to_anchor(a);
        self.back_from_anchor(la);
        let (pb, rb, lb) = self.walk_to_anchor(b);
        self.back_from_anchor(lb);
        let found = self
            .nw
            .block_mut()
            .find_2_slots(pa, ra, pb, rb)
            .map_err(|_| BlockExhausted)?;
        if let Some(g) = found.grew {
            let tr = self.nw.block().translator().clone();
            self.nw.grew_fix(g, &tr);
        }
        let (sa, sb) = (found.slides.a, found.slides.b);
        //apply each at its anchor (re-walked: the path re-derives post-grew; the
        //other's slide keeps this anchor where find_2_slots found it)
        self.walk_to_anchor(a);
        let open_a = self.apply_slide(&sa);
        self.back_from_anchor(la);
        self.walk_to_anchor(b);
        let open_b = self.apply_slide(&sb);
        Ok(((open_a, open_b), OpenFixups { grew: found.grew, slides: found.slides }))
    }
}

impl<'block, NW, B> PreOrderWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block + BlockOps<'block>,
    B::N: Node,
    TreeWalker<PreOrder, NW>: TreeWalk<'block, NW, B>,
    TreeWalker<PreOrder, NW>: TreeWalkHelper<'block, NW, B>,
{
    fn open_here(mut self, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        self.open_at(Anchor::Here { rel })
    }

    fn open_child(mut self, child: ChildPos, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        self.open_at(Anchor::Child { idx: child, rel })
    }

    fn open_2_child(
        mut self,
        ca: ChildPos,
        ra: Rel,
        cb: ChildPos,
        rb: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        self.open_2_at(Anchor::Child { idx: ca, rel: ra }, Anchor::Child { idx: cb, rel: rb })
    }

    ///preorder: a new parent goes before the current node.
    fn open_parent(self) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        self.open_here(Rel::Before)
    }

    fn open_parent_child(
        mut self,
        child: ChildPos,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        self.open_2_at(Anchor::Here { rel: Rel::Before }, Anchor::Child { idx: child, rel })
    }
}

impl<'block, NW, B> InOrderWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<InOrder, NW>: TreeWalk<'block, NW, B>,
    TreeWalker<InOrder, NW>: TreeWalkHelper<'block, NW, B>,
{
    fn open_here(mut self, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        self.open_at(Anchor::Here { rel })
    }

    fn open_child(mut self, child: ChildPos, rel: Rel) -> Result<(OpenSlot, OpenFixups), BlockExhausted> {
        self.open_at(Anchor::Child { idx: child, rel })
    }

    fn open_2_child(
        mut self,
        ca: ChildPos,
        ra: Rel,
        cb: ChildPos,
        rb: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        self.open_2_at(Anchor::Child { idx: ca, rel: ra }, Anchor::Child { idx: cb, rel: rb })
    }

    ///binary contract: ≤ 2 children; the riser must be a leaf or binary — a
    /// unary internal riser has no defined slot 1 (panics by the `child`
    /// contract). child 0 (L) rises, current (P) demotes to L's slot 1, L's
    /// right subtree moves under P's slot 0. nothing moves physically. ends
    /// on L; at the root the block root follows (the riser's own stored parent
    /// field is only written in the non-root case — parent-free shapes don't
    /// carry one).
    fn rotate_right(&mut self) {
        debug_assert!(self.nw.child_count() <= 2, "rotate_right: binary contract");
        //reads (tree valid): L = child 0; LR = L's right subtree
        let l_a = self.nw.child(ChildPos(0));
        let lr_a = {
            self.nw.descend(ChildPos(0)); //walker: -> L
            let lr = if self.nw.is_leaf() {
                None
            } else {
                debug_assert!(
                    self.nw.child_count() == 2,
                    "rotate_right: unary internal riser — slot 1 undefined by contract"
                );
                Some(self.nw.child(ChildPos(1)))
            };
            self.nw.ascend(); //walker: -> P
            lr
        };
        let p_a = self.nw.block().p2a(self.nw.position());
        let parent = self.nw.parent();
        //P-side writes: the grandparent's entry names L; P's slot 0 takes LR
        //(cleared when L was a leaf); P's parent field names L
        if let Some((_, idx)) = parent {
            self.nw.set_child(1, idx, l_a);
        }
        match lr_a {
            Some(lr) => self.nw.set_child(0, ChildPos(0), lr),
            None => self.nw.clear_child(0, ChildPos(0)),
        }
        self.nw.set_parent(l_a);
        //reach L — P's slot 0 no longer names it: via the grandparent's
        //just-written entry, or positionally at the root
        match parent {
            Some((g_pos, idx)) => {
                self.nw.ascend(); //walker: -> G
                self.nw.descend(idx); //walker: -> L
                self.nw.set_parent(self.nw.block().p2a(g_pos));
            }
            None => {
                self.nw.set_position(self.nw.block().a2p(l_a)); //walker: -> L (state only)
            }
        }
        //L-side writes: LR first — L's slot 1 still names it — then it takes P
        if lr_a.is_some() {
            self.nw.descend(ChildPos(1)); //walker: -> LR
            self.nw.set_parent(p_a);
            self.nw.ascend(); //walker: -> L
        }
        self.nw.set_child(0, ChildPos(1), p_a);
        if parent.is_none() {
            let pos = self.nw.position();
            self.nw.block_mut().data_mut().set_root(pos);
        }
    }

    ///mirror: child 1 (R) rises, current (P) demotes to R's slot 0, R's left
    /// subtree moves under P's slot 1. ends on R.
    fn rotate_left(&mut self) {
        debug_assert!(self.nw.child_count() == 2, "rotate_left: needs the right child");
        //reads (tree valid): R = child 1; RL = R's left subtree
        let r_a = self.nw.child(ChildPos(1));
        let rl_a = {
            self.nw.descend(ChildPos(1)); //walker: -> R
            let rl = if self.nw.is_leaf() { None } else { Some(self.nw.child(ChildPos(0))) };
            self.nw.ascend(); //walker: -> P
            rl
        };
        let p_a = self.nw.block().p2a(self.nw.position());
        let parent = self.nw.parent();
        //P-side writes: the grandparent's entry names R; P's slot 1 takes RL
        //(cleared when R was a leaf); P's parent field names R
        if let Some((_, idx)) = parent {
            self.nw.set_child(1, idx, r_a);
        }
        match rl_a {
            Some(rl) => self.nw.set_child(0, ChildPos(1), rl),
            None => self.nw.clear_child(0, ChildPos(1)),
        }
        self.nw.set_parent(r_a);
        //reach R — via the grandparent's just-written entry, or positionally at
        //the root
        match parent {
            Some((g_pos, idx)) => {
                self.nw.ascend(); //walker: -> G
                self.nw.descend(idx); //walker: -> R
                self.nw.set_parent(self.nw.block().p2a(g_pos));
            }
            None => {
                self.nw.set_position(self.nw.block().a2p(r_a)); //walker: -> R (state only)
            }
        }
        //R-side writes: RL first — R's slot 0 still names it — then it takes P
        if rl_a.is_some() {
            self.nw.descend(ChildPos(0)); //walker: -> RL
            self.nw.set_parent(p_a);
            self.nw.ascend(); //walker: -> R
        }
        self.nw.set_child(0, ChildPos(0), p_a);
        if parent.is_none() {
            let pos = self.nw.position();
            self.nw.block_mut().data_mut().set_root(pos);
        }
    }
}

// ---- shared walk helpers (free fns over the consumer walker) ----

///in-order position boundary: the node sits between child[b-1] and child[b],
///`b = min(cc, DEGREE/2)` — after all children when cc ≤ DEGREE/2 (fixed by DEGREE,
///not cc: a full node's boundary is exactly its kept-left-half's edge, so splits
///never move the split node).
fn in_boundary<'block, B: BlockTrait<'block>>(cc: usize) -> ChildPos
where B::N: Node {
    ChildPos(cc.min(<B::N as Node>::DEGREE / 2))
}

fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    while nw.parent().is_some() {
        nw.ascend(); //walker: one level toward the root
    }
}

fn leftmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(ChildPos(0)); //walker: -> child 0
        levels += 1;
    }
    levels
}

fn rightmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    let mut levels = 0;
    while !nw.is_leaf() {
        nw.descend(ChildPos(nw.child_count() - 1)); //walker: -> the last child
        levels += 1;
    }
    levels
}

#[cfg(test)]
#[path = "tests/walker.rs"]
mod tests;