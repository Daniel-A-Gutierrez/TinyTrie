//! the node contract (`Node`/`SplittableNode`) + the three walker layers + the
//! split driver. layer 1 — `NodeCursor`/`NodeWalker`/`NodeWalkerMut`: the
//! consumer-implemented mask over the node representation (the crate never sees
//! union/enum/whatever).
//! layer 2 — `TreeWalker<O, NW>` + `TreeWalk`: ordered
//! traversal, one impl per ordering.
//! layer 3 — `TreeWalkMut` (the tree-level verbs) + `TreeWalkHelper`
//! (crate-internal choreography: slide engine, hop,
//! reparent machinery) + the split machinery: tree ops over the unified
//! `BlockOps` surface. `B` is a trait param
//! at every level; `O` is never a param — it is always `B::O` (the wrapper
//! carries it as phantom data).

use crate::blocks::{BlockOps, BlockTrait, OpenSlot};
use crate::index::Addr;
use crate::metadata::{ChildPos, CursorState, Fixable, Fixup, HasRoot, Pos, SwapFixup};
use crate::store::NoneSlide;
use crate::{InOrder, Order, PostOrder, PreOrder, Rel};
use std::cmp::Ordering;
use std::marker::PhantomData;

///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}

///the insertion-anchor plan: insert can be before or after an existing element, one is cheaper
///to get to - for preorder to insert a child prior to an existing child, left of next is cheaper.
#[derive(Clone, Copy)]
pub enum Suggested {
    ///anchor = the current node (the parent); no walk.
    Parent { rel: Rel },
    ///anchor = child `idx`'s subtree edge — descend `idx`, then `subtree_first`
    ///(Before) / `subtree_last` (After).
    Child { idx: ChildPos, rel: Rel },
}

///`TreeWalkMut::insert_child` failure modes. either we're out of memory in the node or we're out
/// of addresses/memory in the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertErr {
    ///the current node has no room for another child (split the node).
    NodeFull,
    ///the block is exhausted — no slot, no spread, no edge room (split the block).
    BlockExhausted,
}

pub trait Node {
    type K;
    type V;
    type A: Addr;
    ///maximum children per node (in-order layout's parent gap scales with it).
    ///≥ 3 — a full node needs ≥2 keys to split into two non-degenerate halves + a
    ///separator.
    const DEGREE: usize;
    ///do nodes store parent-pointer fields? gates the reparent machinery
    ///(`TreeWalkHelper`: `swap_current`, the NoneSlide fixup, `promote_new_root`);
    ///false shapes pay nothing (the const check folds away).
    /// obligations, all const-gated: slides
    ///⇒ `reparent_run` (in `apply_slide`); swaps ⇒ `swap_current`; fresh/moved
    ///nodes ⇒ `adopt_node` (Y at every split site — its drained children name X;
    ///R at every root promotion — it demotes under NR; the new node at every
    ///insert).
    const STORES_PARENTS: bool;
    ///what a split promotes besides the separator: B-tree internal = the median's V,
    ///B+ inode = `()`.
    type Payload;
}

pub trait SplittableNode: Node + Sized {
    ///the promoted root for `split_root`, **pre-wired with its first child = the old
    ///root** (addr `r_a`). absorbing the child-0 wire here — the only wire with no
    ///separator or promotion — keeps `insert_child`'s arguments non-optional. the
    ///leaf/inode choice is the shape's; `Default` on Node is not trusted to know it.
    ///TODO : doesnt it make sense to take Node::payload here, or at least the same stuff as insert child?
    ///theres no real reason to make the arg strictly non optional, is there?
    fn new_root(r_a: Self::A) -> Self;
    ///drain the right half out of self; self keeps the left half. returns the
    ///separator + payload for the parent's new entry, and the drained right
    ///half — the caller places it into the opened slot.
    fn split(&mut self) -> (Self::K, Self::Payload, Self);
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
    ///the cursor's tracked state — the seam the crate's defaults below run on
    ///(`CursorState`: position + descent record + `Fixable` via supertrait, so
    ///every grow/slide/swap fixup corrects it). PER IMPLEMENTOR: a stackless
    ///cursor picks `Pos`, a stackful walker picks `PosAncestry`.
    type State: CursorState<B::A>;
    ///the state, shared. with `state_mut` these are the only plumbing a consumer
    ///writes for the whole ladder — everything mechanical below is defaulted.
    fn state(&self) -> &Self::State;
    fn state_mut(&mut self) -> &mut Self::State;
    fn block(&self) -> &B;
    fn is_root(&self) -> bool;
    fn is_leaf(&self) -> bool;
    ///number of children of the current node.
    fn child_count(&self) -> usize;
    ///addr of child `child`. contract: the crate's generic code gates every child
    ///access/descent on `is_leaf()` first; this (and the mut-layer child ops)
    ///PANIC when the current node can't support the operation.
    fn child(&self, child: ChildPos) -> B::A;
    ///the current node's child addrs, in order.
    fn children(&self) -> impl Iterator<Item = B::A> + '_ ;
    ///node-level relative position of `k` among the current node's ordered children:
    ///`(child slot, cmp)`. search owns routing.
    fn lookup(&self, k: &<B::N as Node>::K) -> (ChildPos, Ordering);
    ///position of the current node.
    fn position(&self) -> Pos {
        self.state().position()
    }
    fn current<'b>(&'b self) -> &'b B::N
    where 'block: 'b {
        self.block().get(self.state().position())
    }
    ///descend into child `child` — child → `a2p` → the state's descent
    ///record (no-op for a stackless state) + reposition. the walker names the
    ///child afterward.
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b B::N
    where 'block: 'b {
        let child_addr = self.child(child);
        let pos = self.block().a2p(child_addr);
        let parent = self.state().position();
        self.state_mut().descend(parent, child);
        self.state_mut().reposition(pos);
        self.block().get(pos)
    }
    ///descend from the current node using k to lookup a child of the current node repeatedly.
    fn search(&mut self, k: &<B::N as Node>::K) -> Option<&B::N>;
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

///consumer mut surface: node-level reads/writes masked behind the walker.
pub trait NodeWalkerMut<'block, B>: NodeWalker<'block, B>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    ///split-borrow the walker: mutable state + shared block, from ONE call — two
    ///separate accessors would reintroduce the state-vs-block borrow conflict the
    /// fixup path hits (`state.grew_fix(g, block.translator())`).
    fn parts(&mut self) -> (&mut Self::State, &B);

    ///mutable-both split (the `set_child` pa
    fn parts_mut(&mut self) -> (&mut Self::State, &mut B);

    fn block_mut(&mut self) -> &mut B;
    fn current_mut<'b>(&'b mut self) -> &'b mut B::N
    where 'block: 'b {
        let p = self.state().position();
        self.block_mut().get_mut(p)
    }
    ///current node has room for one more child/payload.
    fn has_space(&self) -> bool;
    ///reposition the walker: it names the node at `pos` (no tree meaning — state only).
    fn set_position(&mut self, pos: Pos) {
        self.state_mut().reposition(pos);
    }
    ///set the child at `child` (levels `up`) to `addr`.
    fn set_child(&mut self, up: usize, child: ChildPos, addr: B::A);
    ///set current node's parent field. no-op for parent-free shapes. panic on root.
    fn set_parent(&mut self, addr: B::A);
    ///insert a child into this node.
    fn insert_child(
        &mut self,
        child_idx: ChildPos,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        addr: B::A,
    );
    ///remove child `child_idx`
    fn remove_child(
        &mut self,
        child_idx: ChildPos,
    ) -> (Option<<B::N as Node>::K>, Option<<B::N as Node>::Payload>, B::A);
}

///ordered traversal in the block's layout ordering, over the wrapper.
///`next`/`prev`, the subtree edge walks,
///insertion-anchor suggestion; `first`/`last`.
///depending on the ordering one path is faster for insertion - before next subtree edge or after prev subtree edge
///suggest should let the walker take the shorter path to the correct position.
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
    ///cheapest anchor for a new child at slot `child_idx` (the gap).
    fn suggest_insertion(&self, child_idx: ChildPos) -> Suggested;
    ///the split's target-slot anchor, standing on the split node, `mid = DEGREE>>1`.
    ///childless X → `Parent{After}` for all three orderings.
    /// preorder: before
    ///`child[mid]` (Y placed there, X stays).
    /// in-order: after the rightmost child's
    ///subtree (Y placed there; X never moves — `in_boundary`). postorder: before
    ///`child[mid]` (X relocated there; Y inherits X's slot).
    fn suggest_split(&self) -> Suggested;
}

///layer 3 — crate-internal choreography over the unified `BlockOps` surface: the
///slide engine (`fixup`/`apply_slide` + the slot openers), the in-order hop, and
///the reparent machinery moved off `NodeWalkerMut` — machinery the consumer never
///calls, only the crate's tree ops do (the `SplitWalkHelper` precedent: pub
///in-module, not consumer surface). the per-ordering suggestions/subtree edges
///come in via the `TreeWalk` supertrait. `B::BlockData: HasRoot` — the hop may
///move the block root.
pub trait TreeWalkHelper<'block, NW, B>: TreeWalk<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
{
    ///point the current node's children's stored parent fields at `new_a`.
    ///`STORES_PARENTS`-gated: false shapes return immediately (the const check
    ///folds away). only sound when the current node's child entries are
    ///consistent with the layout — post-swap, post-slide.
    fn reparent_children(&mut self, new_a: B::A);
    ///the slide-companion to `reparent_children`: after `ns` is APPLIED, point each
    ///moved node's children's parent fields at the node's post-slide addr. must run
    ///post-slide — mid-fixup it would descend through just-rewritten (post-slide)
    ///entries over the still-pre-slide layout (subtle_bugs.md §3); post-slide every
    ///entry is consistent (in-run children were rewritten to where they now are,
    ///out-of-run children never moved). position-based over the shifted run — no
    ///tree walk, no collection. position-restoring.
    fn reparent_run(&mut self, ns: &NoneSlide);
    ///finish a freshly created or freshly moved node at position `pos`: its own parent
    ///field points at `parent_a`, its children's stored parent fields point at it.
    ///STORES_PARENTS-gated; position-restoring.
    ///TODO - doc comment is vague
    fn adopt_node(&mut self, pos: Pos, parent_a: B::A);
    ///swap the CURRENT node into the open slot: the node's content moves, the
    ///walker follows (position + ancestry via `SwapFixup`), the parent's entry is
    ///repointed (ancestry-authoritative; skipped at the root), and the node's
    ///children's stored parent fields follow (STORES_PARENTS). returns the
    ///vacated slot. the BLOCK ROOT is not updated — tree-level callers that move
    ///the root do it themselves (`HasRoot`).
    ///
    fn swap_current(&mut self, open: OpenSlot) -> OpenSlot;
    ///run-parent-fixup for a pending slide `ns` — BEFORE the slide is applied, rewrite
    ///each moved node's parent→child pointer (and the moved node's stored parent field
    ///when its parent also moved; no-op for parent-free shapes). the walker must be
    ///positioned at the slide's anchor with valid ancestry. `far_short`: the caller
    ///knows the run walk ends ONE below the far edge — the in-order hop's skewed run
    ///(the misplaced hoppee, when it is the far-edge member, is visited first).
    fn fixup(&mut self, ns: &NoneSlide, far_short: bool);
    ///apply a pending slide: run-parent-fixup → `slide_none` → walker-state fixup →
    ///`reparent_run` (STORES_PARENTS). THE chokepoint — every slide in the tree ops
    ///goes through here. `far_short` as `fixup`. returns the opened slot.
    fn apply_slide(&mut self, ns: &NoneSlide, far_short: bool) -> OpenSlot;
    ///walk to `sug`'s anchor: (anchor position, open side, levels back to the current
    ///node). the walker is left AT the anchor — pair with `back_from_anchor`.
    fn walk_to_anchor(&mut self, sug: Suggested) -> (Pos, Rel, usize);
    ///ascend `levels` — the inverse of `walk_to_anchor`.
    fn back_from_anchor(&mut self, levels: usize);
    ///open a slot adjacent-after the current node (find_slot + grow fixups). ends
    ///standing on the current node.
    fn open_after(&mut self) -> Result<OpenSlot, InsertErr>;
    ///open a slot at `sug`'s anchor (evaluated against the CURRENT node); ends
    ///standing on the current node.
    fn open_suggested(&mut self, sug: Suggested) -> Result<OpenSlot, InsertErr>;
    ///in-order: relocate the CURRENT node (a left child insert/split shifted its
    ///boundary children's identity) to the gap before `child[b]`. ends standing on
    ///it at the new position. the grandparent entry is repointed via `swap_current`
    ///unless this is a tree-parentless node — and if the node is the BLOCK ROOT the
    ///root pointer is repointed (`HasRoot`; subtle_bugs.md §4).
    fn hop_current(&mut self) -> Result<(), InsertErr>;
}

///layer 3 — tree ops, crate-implemented: the tree-level verbs the consumer drives.
///everything else (the slide engine, the reparent machinery, the hop) is
///`TreeWalkHelper` above.
pub trait TreeWalkMut<'block, NW, B>: TreeWalkHelper<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
{
    ///place `node` as a new child of the current node, routed by `k` and `payload` via nodewalker.lookup
    /// `suggest_insertion` → walk to the anchor →
    ///`find_slot` → `apply_slide` → insert → ascend
    ///→ node-level wire (`payload` = the caller's promotion data, passed through).
    ///in-order: a LEFT insert (slot < DEGREE/2) shifts the boundary identity and the
    ///parent hops afterward (same rule as a left split; below DEGREE/2 children the
    ///node sits after-all and absorbs). the walker ends at the parent, post-everything.
    /// TODO
    ///the hop's `BlockExhausted` leaves the tree position-invalid — the block is
    ///genuinely full at that point; the arena tier's cleave-before-hop is future work.
    fn insert_child(
        &mut self,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        node: B::N,
    ) -> Result<B::A, InsertErr>;
    ///remove child `child_idx` of the current node: node-level unwire + block slot free.
    ///returns the removed node and its freed slot. no slide involved — no fixups.
    fn remove_child(&mut self, child_idx: ChildPos) -> (B::N, OpenSlot);
}

///layer 3 — splits (place-then-split driver: no clone, no placeholder — the
///split node drains its right half into the opened slot, after every slide, so
///no orphan is ever unreached by a fixup walk).
pub trait SplitTreeWalker<'block, NW, B>:
    TreeWalkMut<'block, NW, B> + SplitWalkHelper<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
{
    ///split child `child_idx` of the current node in two; wire the new half in at
    ///`child_idx+1`. ends standing on the parent.
    ///`NodeFull` = the parent's child array is full (caller splits a level up first);
    ///`BlockExhausted` = no slot / spread / edge room (caller cleaves the block).
    fn split_child(&mut self, child_idx: ChildPos) -> Result<(), InsertErr>;
    ///split the root: insert a fresh root above it (per-ordering slot — pre/post
    ///swap it into the old root's position so the root pointer and addr stay valid;
    ///in-order repoints). bumps the block's height. returns the old-root→new-root
    ///address remap (`SwapFixup`) for external addr holders (arena parents) —
    ///block data and this walker are already fixed. ends standing on the new root.
    fn split_root(&mut self) -> Result<SwapFixup, InsertErr>;
}

///split machinery — declared here (an inherent impl on the wrapper can't name `B`),
///implemented once for the wrapper where `O`/`B` are both in scope and `self.nw` is
///reachable. not consumer surface. the split machinery binds `Node<A = B::A>`.
///slot-opening/`apply_slide`/`hop_current` live on `TreeWalkHelper` (insert machinery
///the splits borrow).
pub trait SplitWalkHelper<'block, NW, B>: TreeWalkMut<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
{
    ///open the split's target slot at `suggest_split`'s anchor; ends standing on the
    ///split node.
    fn open_split_slot(&mut self) -> Result<OpenSlot, InsertErr>;
    ///open two slots at `sug_a`/`sug_b`'s anchors with independent slides
    ///(`find_2_slots`, composed as one `TwoSlide` fixup): both anchors are walked
    ///and both slides computed pre-mutation (both-slides-before-either,
    ///subtle_bugs.md §2), then applied one at a time — disjointness keeps each
    ///anchor valid across the other's slide, and the run-parent walks interleave
    ///with the slides (they cannot compose: a B-run member's parent may live in
    ///A's run, so walk B must see post-slide-A positions). ends standing on the
    ///current node.
    fn open_two(
        &mut self,
        sug_a: Suggested,
        sug_b: Suggested,
    ) -> Result<(OpenSlot, OpenSlot), InsertErr>;
    ///the split proper for child `child_idx` of the current node: open the target
    ///slot, drain the right half into it, wire at `child_idx+1`. ends on the parent.
    fn split_child_here(&mut self, child_idx: ChildPos) -> Result<(), InsertErr>;
    ///Y = `open`: drain the CURRENT node's right half into it, wire at
    ///`child_idx+1` in the parent one level up. ends on the parent.
    fn split_into_open(&mut self, child_idx: ChildPos, open: OpenSlot)
    -> Result<(), InsertErr>;
    ///place a fresh root above the old one (which the walker stands on). NOT a move
    ///of an existing node: `new_root` is placed and the old root demotes to its
    ///pre-wired child 0. bumps height. where `open` ends up is per-ordering:
    ///in-order — NR takes `open`, R keeps its slot; repoint the root pointer and
    ///step the walker to NR. pre/post — NR is written at `open`, then swapped onto
    ///R's old position (root pointer and addr untouched, walker follows), so R lands
    ///at `open`. ends standing on NR.
    fn promote_new_root(&mut self, open: OpenSlot);
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

    ///gap 0 → after the parent (it precedes everything); mid gap → before `child(k)`
    ///(subtree-first — one descend); append → after the rightmost leaf.
    fn suggest_insertion(&self, child_idx: ChildPos) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 || child_idx == 0 {
            return Suggested::Parent { rel: Rel::After };
        }
        if child_idx < cc {
            Suggested::Child { idx: child_idx, rel: Rel::Before }
        } else {
            Suggested::Child { idx: ChildPos(cc - 1), rel: Rel::After }
        }
    }

    ///Y lands before `child[mid]` (subtree-first); X keeps its slot. a childless
    ///X (leaf) has no subtree — Y lands right after it.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { rel: Rel::After };
        }
        Suggested::Child { idx: ChildPos(cc >> 1), rel: Rel::Before }
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

    ///the node sits in the gap at `b` — a gap AT `b` is the parent's own, so anchor
    ///here (no walk). the side: boundary grows (cc < DEGREE/2, node stays after-all)
    ///⇒ the new subtree lands before the node; boundary fixed (cc ≥ DEGREE/2) ⇒ after.
    ///other gaps → the adjacent child's edge leaf.
    fn suggest_insertion(&self, child_idx: ChildPos) -> Suggested {
        let cc = self.nw.child_count();
        let b = in_boundary::<B>(cc);
        debug_assert!(child_idx <= cc, "suggest_insertion: child_idx out of range");
        if child_idx == b {
            Suggested::Parent {
                rel: if cc < <B::N as Node>::DEGREE / 2 { Rel::Before } else { Rel::After },
            }
        } else if child_idx < b {
            Suggested::Child { idx: child_idx, rel: Rel::Before }
        } else {
            Suggested::Child { idx: child_idx - 1, rel: Rel::After }
        }
    }

    ///Y lands after the rightmost child's subtree (the region end — X, keeping
    ///[0, mid), sits at its own boundary and never moves). a childless X (leaf)
    ///ends its region at itself.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { rel: Rel::After };
        }
        Suggested::Child { idx: ChildPos(cc - 1), rel: Rel::After }
    }
}

impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PostOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PostOrder> + 'block,
    B::N: Node,
{
    ///postorder: subtree then node. next = first (leftmost) node of the next sibling's
    ///subtree, the parent for a last child, None at the root (postorder last).
    fn next<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let (_, idx) = self.nw.parent()?;
        self.nw.ascend(); //walker: -> the parent
        if idx + 1 < self.nw.child_count() {
            self.nw.descend(idx + 1); //walker: -> the next sibling
            leftmost_leaf(&mut self.nw);
        }
        Some(self.nw.current())
    }

    ///mirror: prev = own last child (a child node is its subtree's last), else the
    ///previous sibling node, walking up past first children.
    fn prev<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let cc = self.nw.child_count();
        if cc > 0 {
            self.nw.descend(ChildPos(cc - 1)); //walker: -> own last child
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend(); //walker: -> the parent
            if idx > 0 {
                self.nw.descend(idx - 1); //walker: -> the prev sibling
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

    ///the node follows its subtree.
    fn subtree_first(&mut self) -> usize {
        leftmost_leaf(&mut self.nw)
    }

    fn subtree_last(&mut self) -> usize {
        0
    }

    ///childless → before the parent (it follows everything); gap 0 → before child 0's
    ///subtree (leftmost leaf); gap k → after `child(k-1)` (subtree-last — one descend).
    fn suggest_insertion(&self, child_idx: ChildPos) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { rel: Rel::Before };
        }
        debug_assert!(child_idx <= cc, "suggest_insertion: child_idx out of range");
        if child_idx == 0 {
            Suggested::Child { idx: ChildPos(0), rel: Rel::Before }
        } else {
            Suggested::Child { idx: child_idx - 1, rel: Rel::After }
        }
    }

    ///X relocates to before `child[mid]` (its kept-half region's end); Y inherits
    ///X's vacated slot. a childless X (leaf) keeps its slot — Y lands right after it.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { rel: Rel::After };
        }
        Suggested::Child { idx: ChildPos(cc >> 1), rel: Rel::Before }
    }
}

///generic over `O`: the supertrait obligation (`TreeWalker<O, NW>: TreeWalk`) is
///supplied as a where-clause rather than proven — it only discharges for a concrete
///`B`/`O` pair (one of the per-ordering `TreeWalk` impls), so the coverage is identical
///without needing per-ordering copies of the bodies.
impl<'block, O, NW, B> TreeWalkMut<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn insert_child(
        &mut self,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        node: B::N,
    ) -> Result<B::A, InsertErr> {
        if !self.nw.has_space() {
            return Err(InsertErr::NodeFull);
        }
        //the slot the new child takes: Less ⇒ before child `child`; Equal (k addressed
        //by it) / Greater ⇒ after it
        let (child, cmp) = self.nw.lookup(k);
        let child_idx = child + usize::from(cmp != Ordering::Less);
        //execute the suggestion: walk to the anchor, remember the descent depth
        let (anchor, rel, levels) = match self.suggest_insertion(child_idx) {
            Suggested::Parent { rel } => (self.nw.position(), rel, 0),
            Suggested::Child { idx, rel } => {
                self.nw.descend(idx); //walker: -> the anchor subtree
                let l =
                    if rel == Rel::Before { self.subtree_first() } else { self.subtree_last() };
                (self.nw.position(), rel, 1 + l)
            }
        };
        let found = self.nw.block_mut().find_slot(anchor, rel);
        //the block may have grown (and fixed its own data) even on the exhaustion path —
        //the walker's state must follow either way.
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            let tr = block.translator();
            state.grew_fix(*g, tr);
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        let open = self.apply_slide(&ns, false);
        //place + wire
        self.nw.block_mut().insert(open, node);
        let new_a = self.nw.block().p2a(open.0);
        for _ in 0..levels {
            self.nw.ascend(); //walker: anchor -> back to the current node
        }
        //the new node's own parent field + its (none yet) children (gated)
        self.adopt_node(open.0, self.nw.block().p2a(self.nw.position()));
        self.nw.insert_child(child_idx, k, payload, new_a);
        if O::ORDER == Order::In {
            //a LEFT insert (slot < DEGREE/2) shifted the boundary identity — the
            //parent (current) hops, same rule as a left split; below DEGREE/2
            //children it sits after-all and absorbs.
            let d2 = <B::N as Node>::DEGREE / 2;
            if child_idx < d2 && self.nw.child_count() > d2 {
                self.hop_current()?;
            }
        }
        Ok(new_a)
    }

    fn remove_child(&mut self, child_idx: ChildPos) -> (B::N, OpenSlot) {
        let pos = self.nw.block().a2p(self.nw.child(child_idx));
        let _separator = self.nw.remove_child(child_idx);
        self.nw.block_mut().free(pos)
    }
}

impl<'block, O, NW, B> TreeWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn reparent_children(&mut self, new_a: B::A) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        for idx in 0..self.nw.child_count() {
            self.nw.descend(ChildPos(idx)); //walker: -> child idx
            self.nw.set_parent(new_a);
            self.nw.ascend(); //walker: back
        }
    }

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

    fn adopt_node(&mut self, pos: Pos, parent_a: B::A) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        let back = self.nw.position();
        self.nw.set_position(pos); //walker: -> the fresh/moved node (not yet wired)
        self.nw.set_parent(parent_a);
        self.reparent_children(self.nw.block().p2a(pos));
        self.nw.set_position(back); //walker: restored
    }

    fn swap_current(&mut self, open: OpenSlot) -> OpenSlot {
        let from = self.nw.position();
        let (freed, to) = self.nw.block_mut().swap_open(from, open);
        let sf = SwapFixup { from, to };
        let (state, block) = self.nw.parts();
        let tr = block.translator();
        state.swap_fix(sf, tr);
        if let Some((_, idx)) = self.nw.parent() {
            self.nw.set_child(1, idx, self.nw.block().p2a(to));
        }
        self.reparent_children(self.nw.block().p2a(to));
        freed
    }

    fn fixup(&mut self, ns: &NoneSlide, far_short: bool) {
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
        let snapshot = self.nw.parts().0.clone();
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
        //a ghost lands short/long. the ONE sanctioned skew is the in-order hop's
        //slide (subtle_bugs §11): its misplaced hoppee, when it IS the far-edge
        //member, is visited first, so the walk ends one below — `far_short`.
        let far = if delta > 0 { hi - 1 } else { lo + 1 };
        assert_eq!(
            self.nw.position(),
            far - usize::from(far_short),
            "fixup: run walk endpoint diverged — an occupied slot in the run is \
             unwired (insert without wire?), or the walk was entered mid-run"
        );
        //position-neutral: back at the anchor with entry ancestry — no walking.
        *self.nw.parts().0 = snapshot;
    }

    fn apply_slide(&mut self, ns: &NoneSlide, far_short: bool) -> OpenSlot {
        self.fixup(ns, far_short);
        let open = self.nw.block_mut().slide_none(*ns);
        let (state, block) = self.nw.parts();
        let tr = block.translator();
        state.slide_fix(*ns, tr);
        self.reparent_run(ns);
        open
    }

    fn walk_to_anchor(&mut self, sug: Suggested) -> (Pos, Rel, usize) {
        match sug {
            Suggested::Child { idx, rel } => {
                self.nw.descend(idx); //walker: -> the anchor subtree's root
                let e =
                    if rel == Rel::Before { self.subtree_first() } else { self.subtree_last() };
                (self.nw.position(), rel, 1 + e)
            }
            Suggested::Parent { rel } => (self.nw.position(), rel, 0),
        }
    }

    fn back_from_anchor(&mut self, levels: usize) {
        for _ in 0..levels {
            self.nw.ascend(); //walker: one level toward the current node
        }
    }

    ///open a slot adjacent-after the current node. right-side None: hole at
    /// pos+1, the node stays. left-side fallback (to = pos): the node itself
    /// shifts left one and the hole opens at its old slot — still adjacent-after
    /// the (moved) node; the walker state follows either way. ends standing on
    /// the current node.
    fn open_after(&mut self) -> Result<OpenSlot, InsertErr> {
        let pos = self.nw.position();
        let found = self.nw.block_mut().find_slot(pos, Rel::After);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            state.grew_fix(*g, block.translator());
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        Ok(self.apply_slide(&ns, false))
    }

    fn open_suggested(&mut self, sug: Suggested) -> Result<OpenSlot, InsertErr> {
        let (anchor, rel, levels) = self.walk_to_anchor(sug);
        let found = self.nw.block_mut().find_slot(anchor, rel);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            state.grew_fix(*g, block.translator());
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        let open = self.apply_slide(&ns, false);
        self.back_from_anchor(levels);
        Ok(open)
    }

    fn hop_current(&mut self) -> Result<(), InsertErr> {
        let b = in_boundary::<B>(self.nw.child_count());
        let mut hoppee = self.nw.position();
        //probe from the gap's left edge (after subtree_last(child[b-1])); the
        //hoppee is mid-hop (positionally at its old gap, `in_boundary` already
        //claims the new one), so WHERE the None lands picks the walk's anchor:
        let (anchor, _, levels) =
            self.walk_to_anchor(Suggested::Child { idx: b - 1, rel: Rel::After });
        let found = self.nw.block_mut().find_slot(anchor, Rel::After);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            let tr = block.translator();
            state.grew_fix(*g, tr);
            g.fix_pos(&mut hoppee);
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        let open = if ns.delta <= 0 || ns.from > hoppee {
            //left edge: identity (no walk), None left of the anchor (run below it,
            //walker in-run at hi), or the hoppee INSIDE the run — next() of
            //child[b-1] IS the hoppee via the ancestry stack (no entry read,
            //position-true), and descents after it run through unprocessed entries
            //only (subtle_bugs.md §1: no walk in a diverged window; §5:
            //forward-only soundness). skewed run: when the hoppee is itself the
            //far-edge member it is visited FIRST, so the walk ends one below far.
            let far_short = ns.from == hoppee + 1 && ns.from.0 - ns.to.0 > 1;
            let open = self.apply_slide(&ns, far_short);
            self.back_from_anchor(levels);
            open
        } else {
            //None between the gap and the hoppee: the run is entirely consistent
            //child[b]-side nodes — walk it from its first member
            //(subtree_first(child[b]) sits exactly at the slide's `to`), so the
            //walk never crosses the misplaced hoppee.
            self.back_from_anchor(levels);
            let (anchor_r, rel_r, levels_r) =
                self.walk_to_anchor(Suggested::Child { idx: b, rel: Rel::Before });
            debug_assert!(
                rel_r == Rel::Before && anchor_r == ns.to,
                "hop: child[b] edge != slide to"
            );
            let open = self.apply_slide(&ns, false);
            self.back_from_anchor(levels_r);
            open
        };
        //the block root moves too if the hoppee is parentless yet IS the block root
        //(the walker is back on the hoppee — post-slide position; `swap_current`
        // deliberately leaves the block root to the caller, subtle_bugs.md §4)
        let was_root =
            self.nw.parent().is_none() && self.nw.block().data().root() == self.nw.position();
        self.swap_current(open);
        if was_root {
            self.nw.block_mut().data_mut().set_root(open.0);
        }
        Ok(())
    }
}

impl<'block, O, NW, B> SplitWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn open_split_slot(&mut self) -> Result<OpenSlot, InsertErr> {
        self.open_suggested(self.suggest_split())
    }

    fn open_two(
        &mut self,
        sug_a: Suggested,
        sug_b: Suggested,
    ) -> Result<(OpenSlot, OpenSlot), InsertErr> {
        //both anchors walked (and returned from) before anything mutates
        let (a, aa, la) = self.walk_to_anchor(sug_a);
        self.back_from_anchor(la);
        let (b, ab, lb) = self.walk_to_anchor(sug_b);
        self.back_from_anchor(lb);
        let found = self
            .nw
            .block_mut()
            .find_2_slots(a, aa, b, ab)
            .map_err(|_| InsertErr::BlockExhausted)?;
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            state.grew_fix(*g, block.translator());
        }
        let sa = found.slides.a;
        let sb = found.slides.b;
        //apply each at its anchor (re-walked: the path re-derives post-grew; the
        //other's slide keeps this anchor where find_2_slots found it)
        self.walk_to_anchor(sug_a);
        let open_a = self.apply_slide(&sa, false);
        self.back_from_anchor(la);
        self.walk_to_anchor(sug_b);
        let open_b = self.apply_slide(&sb, false);
        self.back_from_anchor(lb);
        Ok((open_a, open_b))
    }

    ///the split proper for child `child_idx` of the current node: open the target
    ///slot, drain the right half into it, wire at `child_idx+1`. ends on the parent.
    fn split_child_here(&mut self, child_idx: ChildPos) -> Result<(), InsertErr> {
        self.nw.descend(child_idx); //walker: -> the split child X
        let open = self.open_split_slot()?;
        if O::ORDER == Order::Post && self.nw.child_count() > 0 {
            //X (current) relocates into the opened slot via swap_current (state
            //follows, parent entry repointed, children reparented); Y (the drained
            //right half) inherits X's vacated slot.
            let freed = self.swap_current(open);
            let y_a = self.nw.block().p2a(freed.0);
            let x_pos = self.nw.position();
            let (sep, payload, y) = self.nw.block_mut().get_mut(x_pos).split();
            self.nw.block_mut().insert(freed, y);
            //Y's drained children name X — adopt Y under X's parent (gated)
            if let Some((pp, _)) = self.nw.parent() {
                self.adopt_node(freed.0, self.nw.block().p2a(pp));
            }
            self.nw.ascend(); //walker: X (the split node) -> its tree parent
            self.nw.insert_child(child_idx + 1, &sep, payload, y_a);
            Ok(())
        } else {
            //Y = the opened slot; X untouched (preorder: X keeps its slot;
            //in-order: X sits at its boundary — `in_boundary`; postorder leaf:
            //X's region is just itself, right of which Y lands).
            self.split_into_open(child_idx, open)
        }
    }

    fn split_into_open(
        &mut self,
        child_idx: ChildPos,
        open: OpenSlot,
    ) -> Result<(), InsertErr> {
        let x = self.nw.position();
        let y_a = self.nw.block().p2a(open.0);
        let (sep, payload, y) = self.nw.block_mut().get_mut(x).split();
        self.nw.block_mut().insert(open, y);
        //Y's drained children name X — adopt Y under the tree parent (gated)
        if let Some((pp, _)) = self.nw.parent() {
            self.adopt_node(open.0, self.nw.block().p2a(pp));
        }
        self.nw.ascend(); //walker: X (the split node) -> its tree parent
        self.nw.insert_child(child_idx + 1, &sep, payload, y_a);
        Ok(())
    }

    /// in order places the new root at open, pre and post put the old root at open and the new root takes its place.
    fn promote_new_root(&mut self, open: OpenSlot) {
        if O::ORDER == Order::In {
            let r_pos = self.nw.position();
            let r_a = self.nw.block().p2a(r_pos);
            self.nw.block_mut().insert(open, <B::N as SplittableNode>::new_root(r_a));
            let d = self.nw.block_mut().data_mut();
            d.set_root(open.0);
            d.set_height(d.height() + 1);
            self.nw.set_position(open.0); //walker steps to NR (R unreachable through children yet)
            //R keeps its slot but demotes under NR — its parent field names NR
            //(children unchanged; the reparent is idempotent) (gated)
            self.adopt_node(r_pos, self.nw.block().p2a(open.0));
        } else {
            //child 0 = the old root's POST-swap addr (it lands at `open`)
            let r_pos = self.nw.position();
            let r_a = self.nw.block().p2a(open.0);
            self.nw.block_mut().insert(open, <B::N as SplittableNode>::new_root(r_a));
            //raw swap: NR lands at r_pos — the walker's position now names NR
            //(not R); R is at `open`
            self.nw.block_mut().swap(open.0, r_pos);
            let d = self.nw.block_mut().data_mut();
            d.set_height(d.height() + 1);
            //R moved to `open` and demoted under NR: its parent field names NR
            //and its children follow it (gated)
            self.adopt_node(open.0, self.nw.block().p2a(r_pos));
        }
    }
}

impl<'block, O, NW, B> SplitTreeWalker<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn split_child(&mut self, child_idx: ChildPos) -> Result<(), InsertErr> {
        if !self.nw.has_space() {
            return Err(InsertErr::NodeFull);
        }
        self.split_child_here(child_idx)?;
        if O::ORDER == Order::In {
            //a LEFT split (slot < DEGREE/2) shifted the parent's boundary identity
            //and it must hop; below DEGREE/2 children it sits after-all and absorbs.
            let d2 = <B::N as Node>::DEGREE / 2;
            if child_idx < d2 && self.nw.child_count() > d2 {
                self.hop_current()?;
            }
        }
        Ok(())
    }

    fn split_root(&mut self) -> Result<SwapFixup, InsertErr> {
        let r_pos = self.nw.position();
        match O::ORDER {
            //NR takes R's slot (root-first), R lands right of it; the swap keeps
            //the walker on the root and the root addr stable (no-op remap).
            Order::Pre => {
                let open = self.open_after()?;
                self.promote_new_root(open);
                self.split_child_here(ChildPos(0))?;
                Ok(SwapFixup::no_op(r_pos))
            }
            //root-last: NR ends up after everything. INTERNAL R: both slots open
            //up front via find_2_slots (independent slides) while the tree is
            //fully consistent — r_slot at R's kept-half region end (before
            //subtree(mid)), y_slot after child[cc-1] — then drain R into y_slot,
            //then NR into r_slot + swap with R: NR lands under the walker at
            //r_pos, R at r_slot (its post-split position). no walk runs in the
            //transient window between drain and swap (subtle_bugs.md §1).
            //LEAF R: childless — nothing to relocate; Y right after R, NR right
            //after Y; each slot is written before the next opens, so plain
            //sequential opens suffice (and R was the last node, so the run right
            //of Y is empty — the fixup walk is a no-op where Y is still unwired).
            Order::Post => {
                let cc = self.nw.child_count();
                if cc == 0 {
                    let y_open = self.open_split_slot()?; //Parent{After} — right of R
                    let (sep, payload, y) = self.nw.block_mut().get_mut(r_pos).split();
                    self.nw.block_mut().insert(y_open, y);
                    let found = self.nw.block_mut().find_slot(y_open.0, Rel::After);
                    let (mut y_open, mut r_pos) = (y_open, r_pos);
                    if let Some(g) = found.grew.as_ref() {
                        let (state, block) = self.nw.parts();
                        state.grew_fix(*g, block.translator());
                        //the grow moved Y (written) and possibly R — both are
                        //live positions held across this find_slot and must follow
                        g.fix_pos(&mut y_open.0);
                        g.fix_pos(&mut r_pos);
                    }
                    let Some(ns) = found.slide else {
                        return Err(InsertErr::BlockExhausted);
                    };
                    //Y is unwired — a fixup walk is only sound because the run is
                    //empty, which the root-last invariant guarantees.
                    debug_assert!(
                        ns.from == ns.to,
                        "split_root(post,leaf): nonempty run right of Y"
                    );
                    let nr_open = self.nw.block_mut().slide_none(ns);
                    let nr = <B::N as SplittableNode>::new_root(self.nw.block().p2a(r_pos));
                    self.nw.block_mut().insert(nr_open, nr);
                    {
                        let d = self.nw.block_mut().data_mut();
                        d.set_root(nr_open.0);
                        d.set_height(d.height() + 1);
                    }
                    self.nw.set_position(nr_open.0); //postorder's one bend (leaf root)
                    //R (leaf, unmoved) and Y (fresh) both demote under NR (gated —
                    //leaf R has no children; Y's field is the only work)
                    self.adopt_node(r_pos, self.nw.block().p2a(nr_open.0));
                    self.adopt_node(y_open.0, self.nw.block().p2a(nr_open.0));
                    self.nw.insert_child(
                        ChildPos(1),
                        &sep,
                        payload,
                        self.nw.block().p2a(y_open.0),
                    );
                    //the root addr moves — a real (non no-op) remap
                    Ok(SwapFixup { from: r_pos, to: nr_open.0 })
                } else {
                    let (r_slot, y_slot) = self.open_two(
                        self.suggest_split(),
                        Suggested::Child { idx: ChildPos(cc - 1), rel: Rel::After },
                    )?;
                    //R may have moved with open_two's slides (the walker followed
                    //it) — the pre-open r_pos is stale; reread
                    let r_pos = self.nw.position();
                    let (sep, payload, y) = self.nw.block_mut().get_mut(r_pos).split();
                    self.nw.block_mut().insert(y_slot, y);
                    let nr = <B::N as SplittableNode>::new_root(self.nw.block().p2a(r_slot.0));
                    self.nw.block_mut().insert(r_slot, nr);
                    //raw swap: NR lands at r_pos — the walker's position now
                    //names NR (not R); R is at r_slot
                    self.nw.block_mut().swap(r_slot.0, r_pos);
                    let d = self.nw.block_mut().data_mut();
                    d.set_height(d.height() + 1);
                    //R moved to r_slot, Y fresh at y_slot — both demote under NR,
                    //Y's drained children name R (gated)
                    self.adopt_node(r_slot.0, self.nw.block().p2a(r_pos));
                    self.adopt_node(y_slot.0, self.nw.block().p2a(r_pos));
                    self.nw.insert_child(
                        ChildPos(1),
                        &sep,
                        payload,
                        self.nw.block().p2a(y_slot.0),
                    );
                    Ok(SwapFixup::no_op(r_pos))
                }
            }
            //R KEEPS its slot (its valid range is the single gap between
            // subtree(DEGREE/2-1) and subtree(DEGREE/2), unchanged by the split), so
            // no swap can put NR after R under the walker's feet — the one
            // sanctioned `set_position`. NR's slot per its own convention
            // (b = min(2, DEGREE/2)): between its children (right of R) when
            // b == 1, after-all (the region end) when b == 2.
            Order::In => {
                let d2 = <B::N as Node>::DEGREE / 2;
                //childless R: NR's boundary b = min(1, d2) == cc — after-all, so
                //NR lands right of R (region end). d2 < 2 same: b == cc always.
                let open = if self.nw.child_count() == 0 || d2 < 2 {
                    self.open_after()?
                } else {
                    self.open_suggested(Suggested::Child {
                        idx: ChildPos(self.nw.child_count() - 1),
                        rel: Rel::After,
                    })?
                };
                self.promote_new_root(open);
                self.split_child_here(ChildPos(0))?;
                //the root addr moves — a real (non no-op) remap. `to` from the
                //walker's live position: the child-split's slide may have moved
                //NR off `open` (state + block data are fixed; `open.0` is stale)
                Ok(SwapFixup { from: r_pos, to: self.nw.position() })
            }
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

//tests unwired for the addr/pos terminology refactor (Pos/ChildPos/Rel signatures)
//— port src/tests/walker.rs to the new surface, then re-enable:
//#[cfg(test)]
//#[path = "tests/walker.rs"]
//mod tests;
