//! the node contract (`Node`) + the walker layers. layer 1 — `NodeCursor`/
//! `NodeWalker`/`NodeWalkerMut`: the consumer-implemented mask over the node
//! representation; the walker IS its own state (`Fixable`), and the crate
//! reads `position` and moves the walker only through `descend`/`ascend` and
//! the internal state ops. layer 2 — `TreeWalker<O, NW>` + `TreeWalk`: ordered
//! traversal, one impl per ordering (pre, in; post unimplemented).
//! layer 3 — `PreOrderWalk`/`InOrderWalk`: the open surface. slot-moving ops
//! CONSUME the walker — a stale position is unrepresentable at the mutation
//! point; rotations keep it (nothing moves physically, and the stand-on-the-
//! riser contract ends where it started). `open_n_*` = the N-None gather
//! (one scattered-holes compaction per open). wiring is
//! consumer-side: opens return slots + the applied fixups, the consumer
//! inserts and rewires via the block directly. `B` is a trait param at every
//! level; `O` is always `B::O` (the wrapper carries it as phantom data).
//! NOTE: traversal assumes packed ChildPos (rank == slot); sparse-addressed
//! nodes need children()-based sibling walks, unimplemented.

use crate::blocks::{BlockOps, BlockTrait, OpenSlot};
use crate::index::Addr;
use crate::metadata::{ChildPos, DoubleSlide, Fixable, GatherSlide, GrewFixup, HasRoot, Pos};
use crate::store::{NoneSlide, Store};
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
/// for single opens), or — for an `open_n_*` — the one gather. consumers
/// holding addresses/positions across the open apply these via their
/// `Fixable` impl — held addrs need only the slides/gather (addrs are
/// grow-stable by construction), held positions need both, `grew` FIRST (the
/// slides/gather deltas are post-grew coordinates).
#[derive(Clone)]
pub struct OpenFixups {
    pub grew:   Option<GrewFixup>,
    pub slides: DoubleSlide,
    ///Some ⇒ the slots came from one gather; `slides` are no-ops.
    pub gather: Option<GatherSlide>,
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
    ///set the current node's stored parent field, returning the overwritten old
    /// parent addr (`None` for parent-free shapes — the read is free in the
    /// read-modify-write). no-op for parent-free shapes.
    fn set_parent(&mut self, addr: B::A) -> Option<B::A>;
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
    ///n contiguous slots at the current node's position, on the `rel` side —
    /// one gather (scattered Nones crossed by a single move).
    fn open_n_here(
        self,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///n contiguous slots at child `child`'s subtree edge (Before/After the
    /// whole subtree), on the `rel` side — one gather.
    fn open_n_child(
        self,
        child: ChildPos,
        n: usize,
        rel: Rel,
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
    ///n contiguous slots at the current node's position, on the `rel` side —
    /// one gather (scattered Nones crossed by a single move).
    fn open_n_here(
        self,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///n contiguous slots at child `child`'s subtree edge (Before/After the
    /// whole subtree), on the `rel` side — one gather.
    fn open_n_child(
        self,
        child: ChildPos,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///stand-on-the-riser: the walker starts on the riser (its parent's child
    /// 0) and ends on it — the riser rises, the parent demotes to the riser's
    /// slot 1, the riser's right subtree moves under the parent's slot 0.
    /// nothing moves physically; writes via the ancestry stack; the end state
    /// pops one level (the riser rose, so its true path is shallower than its
    /// descent history). at the root the block root follows.
    fn rotate_right(&mut self);
    ///mirror: the riser (its parent's child 1) rises, the parent demotes to
    /// its slot 0, the riser's left subtree moves under the parent's slot 1.
    /// ends on the riser.
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
    ///reparent the children of every Some-dense post-move member slot
    /// `[lo, hi]` (closed). position-based over the shifted layout — no tree
    /// walk (subtle_bugs §3). position-restoring. STORES_PARENTS-gated.
    fn reparent_range(&mut self, lo: Pos, hi: Pos);
    fn fixup(&mut self, ns: &NoneSlide);
    ///run-parent-fixup for a pending gather — pre-apply, one walk (v1: all
    /// holes on one side ⇒ one member interval beside the anchor).
    fn fixup_gather(&mut self, g: &GatherSlide);
    fn apply_slide(&mut self, ns: &NoneSlide) -> OpenSlot;
    ///apply a pending gather: fixup_gather → `gather_none` → walker-state
    /// fixup → reparent over the Some-dense post-gather range.
    fn apply_gather(&mut self, g: &GatherSlide) -> (Pos, Pos);
    fn walk_to_anchor(&mut self, anchor: Anchor) -> (Pos, Rel, usize);
    fn back_from_anchor(&mut self, levels: usize);
    fn open_at(&mut self, anchor: Anchor) -> Result<(OpenSlot, OpenFixups), BlockExhausted>;
    fn open_2_at(
        &mut self,
        a: Anchor,
        b: Anchor,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted>;
    ///n contiguous slots at `anchor` — one gather. the walker's end position is
    /// meaningless to the caller (consumed).
    fn open_n_at(
        &mut self,
        anchor: Anchor,
        n: usize,
    ) -> Result<((Pos, Pos), OpenFixups), BlockExhausted>;
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
        //post-slide member range: delta>0 ⇒ (lo, hi]; delta<0 ⇒ [lo, hi)
        if ns.delta > 0 {
            self.reparent_range(lo + 1, hi);
        } else {
            self.reparent_range(lo, hi - 1);
        }
    }

    fn reparent_range(&mut self, lo: Pos, hi: Pos) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        let back = self.nw.position();
        for q in lo.0..=hi.0 {
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

    ///run-parent-fixup for a pending gather, BEFORE it is applied — same
    ///instrument as `fixup`: rewrite each moved member's parent→child entry
    /// (and its stored parent field, via the parent's own delta — 0 when the
    /// parent didn't move) over the still-valid layout. v1: all holes on the
    /// open side ⇒ the members are the interval's Somes beside the anchor and
    /// the walk is single-direction, forward-only (§5: the entries read on the
    /// way are only ever unprocessed ones). the walker must sit at the anchor —
    /// never a member (the anchor doesn't move).
    fn fixup_gather(&mut self, g: &GatherSlide) {
        let (lo, hi) = match g.rel {
            //the member interval, open: (anchor, q_hi) / (q_lo, anchor) — Before
            //extends TO the anchor-adjacent slot (it holds a member whenever the
            //nearest hole isn't adjacent)
            Rel::After => (g.anchor + 1, *g.holes.last().expect("gather: no holes")),
            Rel::Before => (*g.holes.first().expect("gather: no holes"), g.anchor),
        };
        //steps + far edge: the members are exactly the interval's Somes —
        //the chosen holes are its only other slots
        let (mut steps, mut far) = (0usize, None);
        {
            let block = self.nw.block();
            for s in lo.0..hi.0 {
                if block.store().slot(Pos(s)).is_some() {
                    steps += 1;
                    //After walks up (far = topmost); Before walks down (far = bottommost)
                    if g.rel == Rel::After || far.is_none() {
                        far = Some(Pos(s));
                    }
                }
            }
        }
        if steps == 0 {
            return; //holes adjacent to the anchor — nothing to fix
        }
        //snapshot at the anchor, restored after — as `fixup` (§5): a walk back
        //would descend through just-rewritten entries
        let snapshot = self.nw.save();
        let n = if g.rel == Rel::After { self.next() } else { self.prev() };
        debug_assert!(n.is_some(), "fixup_gather: walk fell off the block");
        for i in 0..steps {
            let p = self.nw.position();
            //per-visit canary: the interval's Some slots are exactly the
            //members, so a visit outside it names an occupied slot no walk can
            //reach — an unwired ghost (subtle_bugs §6)
            let in_interval = if g.rel == Rel::After {
                lo <= p && p < hi
            } else {
                lo < p && p < hi
            };
            assert!(
                in_interval,
                "fixup_gather: walk left the member interval — an occupied \
                 slot in the interval is unwired (insert without wire?)"
            );
            if let Some((ppos, idx)) = self.nw.parent() {
                //delta is 0 for non-members — the parent's own remap covers
                //both moved and unmoved parents
                let pa = ppos.wrapping_add(g.delta(ppos) as usize);
                self.nw.set_parent(self.nw.block().p2a(pa));
                let new_a = self.nw.block().p2a(p.wrapping_add(g.delta(p) as usize));
                self.nw.set_child(1, idx, new_a);
            }
            if i + 1 < steps {
                let n = if g.rel == Rel::After { self.next() } else { self.prev() };
                debug_assert!(n.is_some(), "fixup_gather: walk fell off the block");
            }
        }
        //endpoint canary: against a consistent layout the walk visits the
        //members in slot order — a ghost lands short/long (§6)
        assert_eq!(
            self.nw.position(),
            far.expect("fixup_gather: steps > 0 with no member"),
            "fixup_gather: walk endpoint diverged — an occupied slot in the \
             interval is unwired (insert without wire?)"
        );
        self.nw.load(snapshot);
    }

    fn apply_gather(&mut self, g: &GatherSlide) -> (Pos, Pos) {
        self.fixup_gather(g);
        let slots = self.nw.block_mut().gather_none(g);
        let tr = self.nw.block().translator().clone();
        self.nw.gather_fix(g, &tr);
        //post-gather members are Some-dense: After fills [anchor+n+1, q_hi];
        //Before fills [q_lo, anchor-n-1]
        let n = g.holes.len();
        match g.rel {
            Rel::After => {
                let hi = *g.holes.last().expect("gather: no holes");
                self.reparent_range(g.anchor + n + 1, hi);
            }
            Rel::Before => {
                let lo = *g.holes.first().expect("gather: no holes");
                //no members ⇒ anchor == q_lo+n ⇒ the end would underflow; skip
                let end = g.anchor.0.saturating_sub(n + 1);
                if end >= lo.0 {
                    self.reparent_range(lo, Pos(end));
                }
            }
        }
        slots
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
        Ok((open, OpenFixups { grew: found.grew, slides: DoubleSlide { a: ns, b: noop }, gather: None }))
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
        Ok(((open_a, open_b), OpenFixups { grew: found.grew, slides: found.slides, gather: None }))
    }

    ///n slots at `anchor`: walk, find_n_slots (budgeted → full-len → spread),
    ///grew-fix the walker, one gather. as `open_at`, the walker state's
    /// grew_fix keeps the anchor consistent — no re-walk needed.
    fn open_n_at(
        &mut self,
        anchor: Anchor,
        n: usize,
    ) -> Result<((Pos, Pos), OpenFixups), BlockExhausted> {
        assert!(n > 0, "open_n_at: n == 0");
        let (pos, rel, _) = self.walk_to_anchor(anchor);
        let found = self
            .nw
            .block_mut()
            .find_n_slots(pos, rel, n)
            .map_err(|_| BlockExhausted)?;
        if let Some(gr) = found.grew {
            let tr = self.nw.block().translator().clone();
            self.nw.grew_fix(gr, &tr);
        }
        let slots = self.apply_gather(&found.gather);
        let noop = NoneSlide { from: slots.0, to: slots.0, delta: 0 };
        Ok((
            slots,
            OpenFixups {
                grew:   found.grew,
                slides: DoubleSlide { a: noop, b: noop },
                gather: Some(found.gather),
            },
        ))
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

    fn open_n_here(
        mut self,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        let ((lo, hi), fixups) = self.open_n_at(Anchor::Here { rel }, n)?;
        Ok(((OpenSlot(lo), OpenSlot(hi)), fixups))
    }

    fn open_n_child(
        mut self,
        child: ChildPos,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        let ((lo, hi), fixups) = self.open_n_at(Anchor::Child { idx: child, rel }, n)?;
        Ok(((OpenSlot(lo), OpenSlot(hi)), fixups))
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

    fn open_n_here(
        mut self,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        let ((lo, hi), fixups) = self.open_n_at(Anchor::Here { rel }, n)?;
        Ok(((OpenSlot(lo), OpenSlot(hi)), fixups))
    }

    fn open_n_child(
        mut self,
        child: ChildPos,
        n: usize,
        rel: Rel,
    ) -> Result<((OpenSlot, OpenSlot), OpenFixups), BlockExhausted> {
        let ((lo, hi), fixups) = self.open_n_at(Anchor::Child { idx: child, rel }, n)?;
        Ok(((OpenSlot(lo), OpenSlot(hi)), fixups))
    }

    ///stand-on-the-riser contract: starts on L (child 0), ends on L. reads
    /// first (tree valid), then one ascend-to-P visit — P's field names L, G's
    /// entry follows or the block root does when P was the root — and back via
    /// P's slot 0, which still names L. L's own parent field is only written in
    /// the non-root case (parent-free shapes don't carry one).
    fn rotate_right(&mut self) {
        debug_assert!(self.nw.child_count() <= 2, "rotate_right: binary contract");
        debug_assert!(
            matches!(self.nw.parent(), Some((_, ChildPos(0)))),
            "rotate_right: stand on the riser (parent's child 0)"
        );
        //reads (tree valid): LR = L's right subtree
        let l_pos = self.nw.position();
        let l_a = self.nw.block().p2a(l_pos);
        let (p_pos, _) = self
            .nw
            .parent()
            .expect("rotate_right: the riser has no parent — not the root");
        let p_a = self.nw.block().p2a(p_pos);
        let lr_a = if self.nw.is_leaf() {
            None
        } else {
            debug_assert!(
                self.nw.child_count() == 2,
                "rotate_right: unary internal riser — slot 1 undefined by contract"
            );
            Some(self.nw.child(ChildPos(1)))
        };
        self.nw.ascend(); //walker: -> P
        self.nw.set_parent(l_a); //P's field names L
        let g_a = match self.nw.parent() {
            Some((g_pos, gp_idx)) => {
                self.nw.set_child(1, gp_idx, l_a); //G's entry takes L
                Some(self.nw.block().p2a(g_pos))
            }
            None => {
                self.nw.block_mut().data_mut().set_root(l_pos); //P was root
                None
            }
        };
        self.nw.descend(ChildPos(0)); //walker: -> L (P's slot 0 still names it)
        if let Some(g_a) = g_a {
            self.nw.set_parent(g_a); //L's field names G
        }
        //LR first — L's slot 1 still names it — then P's slot 0, then L takes P
        if lr_a.is_some() {
            self.nw.descend(ChildPos(1)); //walker: -> LR
            self.nw.set_parent(p_a);
            self.nw.ascend(); //walker: -> L
        }
        match lr_a {
            Some(lr) => self.nw.set_child(1, ChildPos(0), lr), //P's slot 0 takes LR
            None => self.nw.clear_child(1, ChildPos(0)), //riser was a leaf
        }
        self.nw.set_child(0, ChildPos(1), p_a); //L's slot 1 = P
        //end-state restore: the riser ROSE a level, so its true path is one
        //shallower than the descent history — pop the transient entry and
        //reposition. state-only over a fully consistent tree (not a mid-op
        //reach). root case: the stack empties, L is the root.
        self.nw.ascend(); //walker: -> P (pops the entry P's old slot-0 named)
        self.nw.set_position(l_pos); //walker: -> L, the riser
    }

    ///mirror (0↔1): starts on R (child 1), ends on R. R rises, P demotes to
    /// R's slot 0, R's left subtree moves under P's slot 1.
    fn rotate_left(&mut self) {
        debug_assert!(self.nw.child_count() <= 2, "rotate_left: binary contract");
        debug_assert!(
            matches!(self.nw.parent(), Some((_, ChildPos(1)))),
            "rotate_left: stand on the riser (parent's child 1)"
        );
        //reads (tree valid): RL = R's left subtree
        let r_pos = self.nw.position();
        let r_a = self.nw.block().p2a(r_pos);
        let (p_pos, _) = self
            .nw
            .parent()
            .expect("rotate_left: the riser has no parent — not the root");
        let p_a = self.nw.block().p2a(p_pos);
        let rl_a = if self.nw.is_leaf() {
            None
        } else {
            Some(self.nw.child(ChildPos(0))) //packed ⇒ slot 0 present when internal
        };
        self.nw.ascend(); //walker: -> P
        self.nw.set_parent(r_a); //P's field names R
        let g_a = match self.nw.parent() {
            Some((g_pos, gp_idx)) => {
                self.nw.set_child(1, gp_idx, r_a); //G's entry takes R
                Some(self.nw.block().p2a(g_pos))
            }
            None => {
                self.nw.block_mut().data_mut().set_root(r_pos); //P was root
                None
            }
        };
        self.nw.descend(ChildPos(1)); //walker: -> R (P's slot 1 still names it)
        if let Some(g_a) = g_a {
            self.nw.set_parent(g_a); //R's field names G
        }
        if rl_a.is_some() {
            self.nw.descend(ChildPos(0)); //walker: -> RL
            self.nw.set_parent(p_a);
            self.nw.ascend(); //walker: -> R
        }
        match rl_a {
            Some(rl) => self.nw.set_child(1, ChildPos(1), rl), //P's slot 1 takes RL
            None => self.nw.clear_child(1, ChildPos(1)), //riser was a leaf
        }
        self.nw.set_child(0, ChildPos(0), p_a); //R's slot 0 = P
        //end-state restore (as rotate_right): pop the transient entry — the
        //riser rose a level; its true path is one shallower
        self.nw.ascend(); //walker: -> P (pops the entry P's old slot-1 named)
        self.nw.set_position(r_pos); //walker: -> R, the riser
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