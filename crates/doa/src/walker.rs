//! the node contract (`Node`/`SplittableNode`) + the three walker layers + the
//! split driver. layer 1 — `NodeCursor`/`NodeWalker`/`NodeWalkerMut`: the
//! consumer-implemented mask over the node representation (the crate never sees
//! union/enum/whatever). layer 2 — `TreeWalker<O, NW>` + `TreeWalk`: ordered
//! traversal, one impl per ordering. layer 3 — `TreeWalkMut` (the tree-level
//! verbs) + `TreeWalkHelper` (crate-internal choreography: slide engine, hop,
//! reparent machinery) + the split machinery: tree ops over the unified
//! `BlockOps` surface. `B` is a trait param
//! at every level; `O` is never a param — it is always `B::O` (the wrapper
//! carries it as phantom data). no `insert_child` name collision:
//! `NodeWalkerMut` is impl'd on the consumer's `NW`, `TreeWalkMut` on the
//! wrapper — different `Self` types, method sets never intersect.

use crate::blocks::{BlockOps, BlockTrait, OpenSlot};
use crate::index::BlockIndex;
use crate::metadata::{CursorState, Fixable, Fixup, HasRoot, SwapFixup};
use crate::store::{NoneSlide, Store};
use crate::{InOrder, Order, PostOrder, PreOrder};
use std::cmp::Ordering;
use std::marker::PhantomData;
use std::mem::MaybeUninit;

///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}

///the insertion-anchor plan: the cheapest name for the gap a new child at slot
///`child_idx` occupies (between `child[idx-1]` and `child[idx]`). pure choice, no
///walking — `TreeWalkMut::insert_child` executes it.
#[derive(Clone, Copy)]
pub enum Suggested {
    ///anchor = the current node (the parent); no walk.
    Parent { before: bool },
    ///anchor = child `idx`'s subtree edge — descend `idx`, then `subtree_first`
    ///(before) / `subtree_last` (after).
    Child { idx: usize, before: bool },
}

///`TreeWalkMut::insert_child` failure modes. the caller handles both by splitting
///(a level up / the root) and retrying; `BlockExhausted` remains the arena tier's
///cleave hook.
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
    type P: BlockIndex;
    ///maximum children per node (in-order layout's parent gap scales with it).
    ///≥ 3 — a full node needs ≥2 keys to split into two non-degenerate halves + a
    ///separator.
    const DEGREE: usize;
    ///do nodes store parent-pointer fields? gates the reparent machinery
    ///(`TreeWalkHelper`: `swap_current`, the NoneSlide fixup, `promote_new_root`);
    ///false shapes pay
    ///nothing (the const check folds away). obligations, all const-gated: slides
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
    ///root** (vaddr `r_v`). absorbing the child-0 wire here — the only wire with no
    ///separator or promotion — keeps `insert_child`'s arguments non-optional. the
    ///leaf/inode choice is the shape's; `Default` on Node is not trusted to know it.
    fn new_root(r_v: Self::P) -> Self;
    ///drain the right half directly into `slot` — a reserved, uninitialized block
    ///place; write into it (no by-value round-trip, no occupancy flag to flip).
    ///self keeps the left half. returns the separator + promotion data for the
    ///parent's new entry.
    fn split(&mut self, slot: &mut MaybeUninit<Self>) -> (Self::K, Self::Payload);
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
    type State: CursorState<B::P>;
    ///the state, shared. with `state_mut` these are the only plumbing a consumer
    ///writes for the whole ladder — everything mechanical below is defaulted.
    fn state(&self) -> &Self::State;
    fn state_mut(&mut self) -> &mut Self::State;

    fn block(&self) -> &B;
    fn is_leaf(&self) -> bool;
    ///number of children of the current node.
    fn child_count(&self) -> usize;
    ///vaddr of child `idx`. contract: the crate's generic code gates every child
    ///access/descent on `is_leaf()` first; this (and the mut-layer child ops)
    ///PANIC when the current node can't support the operation.
    fn child(&self, idx: usize) -> B::P;
    ///the current node's child vaddrs, in order.
    fn children(&self) -> impl Iterator<Item = B::P> + '_ {
        (0..self.child_count()).map(|i| self.child(i))
    }
    ///node-level relative position of `k` among the current node's ordered children:
    ///`(pos, cmp)`. `pos` = the child `search` descends to by default; `cmp` = `k`'s
    ///relation to that child — `Less` = before it (a new child takes slot `pos`),
    ///`Equal` = addressed by it / within its key span (slot `pos+1`), `Greater` = after
    /// it, before child `pos+1` (slot `pos+1`; `(len-1, Greater)` is the append case).
    ///routing is NOT decided here — `search` owns it, and the interpretation of the
    ///pair is the consumer's routing policy (a value-storing inode stops on `Equal`,
    ///B+ equal-right descends), so `search` has no default.
    fn lookup(&self, k: &<B::N as Node>::K) -> (usize, Ordering);
    ///phys of the current node.
    fn position(&self) -> usize {
        self.state().position()
    }
    fn current<'b>(&'b self) -> &'b B::N
    where 'block: 'b {
        self.block().get(self.state().position())
    }
    ///descend into child `child_idx` — child → `v2p` → the state's descent
    ///record (no-op for a stackless state) + reposition.
    fn descend<'b>(&'b mut self, child_idx: usize) -> &'b B::N
    where 'block: 'b {
        let child = self.child(child_idx);
        let phys = self.block().v2p(child);
        let parent = self.state().position();
        self.state_mut().descend(parent, child_idx);
        self.state_mut().reposition(phys);
        self.block().get(phys)
    }
    ///descend from the current node to `k`'s terminal (None conventionally = empty
    ///block). consumer-implemented routing over `lookup`'s `(pos, cmp)` — equal-right,
    ///Eq-stop, …: baking in a default would fix the meaning of `Equal`, which is a
    ///per-shape decision.
    fn search(&mut self, k: &<B::N as Node>::K) -> Option<&B::N>;
}

///ascend-capable cursor — the consumer's stackful walker. `ascend`/`parent` are
///required consumer methods because where parent knowledge lives is per-shape:
///a stackful state pops/peeks its records (`PosAncestry`), a parent-pointer tree
///reads the node's stored field — and a stackful ascend must consume a record
///where a pointer ascend must not, so no default can serve both.
pub trait NodeWalker<'block, B>: NodeCursor<'block, B>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    ///move to the parent node. at the root this is a contract violation (panic).
    fn ascend<'b>(&'b mut self) -> &'b B::N
    where 'block: 'b;
    ///(parent phys, child idx we descended through). None at the root. the idx
    ///feeds the fixup path's `set_child` — parent-pointer shapes that can't know
    ///it without a scan choose their own strategy there.
    fn parent(&self) -> Option<(usize, usize)>;
}

///consumer mut surface: node-level reads/writes masked behind the walker.
pub trait NodeWalkerMut<'block, B>: NodeWalker<'block, B>
where
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    ///split-borrow the walker: mutable state + shared block, from ONE call — two
    ///separate accessors would reintroduce the state-vs-block borrow conflict the
    /// fixup path hits (`state.fixup(f, block.translator())`).
    fn parts(&mut self) -> (&mut Self::State, &B);
    ///mutable-both split (the `set_child` path).
    fn parts_mut(&mut self) -> (&mut Self::State, &mut B);

    fn block_mut(&mut self) -> &mut B;
    fn current_mut<'b>(&'b mut self) -> &'b mut B::N
    where 'block: 'b {
        let p = self.state().position();
        self.block_mut().get_mut(p)
    }
    ///reposition to phys `p` without walking (ancestry untouched — the caller
    ///guarantees consistency; root promotion uses it: the new root is not yet
    ///reachable through children).
    fn set_position(&mut self, p: usize) {
        self.state_mut().reposition(p);
    }
    ///current node has room for one more child/payload.
    fn has_space(&self) -> bool;
    ///set the child pointer `child_idx` of the ancestor `up` levels above the current
    ///node (0 = current) to `ptr`. ancestry-aware, position-stable — the fixup path
    ///rewrites a parent's entry while standing on the child (a walk back down would
    ///descend through the just-rewritten pointer, which pre-slide names the wrong slot).
    fn set_child(&mut self, up: usize, child_idx: usize, ptr: B::P);
    ///set current node's parent field. no-op for parent-free shapes.
    fn set_parent(&mut self, ptr: B::P);
    ///node-level wire: place the entry for a new child at slot `child_idx` —
    ///bounding separator, split-promotion data, child vaddr. no block interaction.
    fn insert_child(
        &mut self,
        child_idx: usize,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        ptr: B::P,
    );
    ///node-level unwire: remove child `child_idx` — returns its bounding separator
    ///(`None` = child 0), promotion data, and vaddr.
    fn remove_child(
        &mut self,
        child_idx: usize,
    ) -> (Option<<B::N as Node>::K>, Option<<B::N as Node>::Payload>, B::P);
}

///ordered traversal in the block's layout ordering, over the wrapper; one impl per
///ordering. the per-ordering contributions: `next`/`prev`, the subtree edge walks,
///and the insertion-anchor suggestion; `first`/`last` are at_root + a subtree edge.
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
    fn suggest_insertion(&self, child_idx: usize) -> Suggested;
    ///the split's target-slot anchor, standing on the split node, `mid = cc>>1`.
    ///childless X → `Parent{after}` for all three orderings. preorder: before
    ///`child[mid]` (Y placed there, X stays). in-order: after the rightmost child's
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
    B::BlockData: HasRoot<B::P>,
{
    ///point the current node's children's stored parent fields at `new_v`.
    ///`STORES_PARENTS`-gated: false shapes return immediately (the const check
    ///folds away). only sound when the current node's child entries are
    ///consistent with the layout — post-swap, post-slide.
    fn reparent_children(&mut self, new_v: B::P);
    ///the slide-companion to `reparent_children`: after `ns` is APPLIED, point each
    ///moved node's children's parent fields at the node's post-slide vaddr. must run
    ///post-slide — mid-fixup it would descend through just-rewritten (post-slide)
    ///entries over the still-pre-slide layout (subtle_bugs.md §3); post-slide every
    ///entry is consistent (in-run children were rewritten to where they now are,
    ///out-of-run children never moved). position-based over the shifted run — no
    ///tree walk, no collection. position-restoring.
    fn reparent_run(&mut self, ns: &NoneSlide);
    ///finish a freshly created or freshly moved node at phys `p`: its own parent
    ///field points at `parent_v`, its children's stored parent fields point at it.
    ///STORES_PARENTS-gated; position-restoring.
    fn adopt_node(&mut self, p: usize, parent_v: B::P);
    ///swap the CURRENT node into the open slot: the node's content moves, the
    ///walker follows (position + ancestry via `SwapFixup`), the parent's entry is
    ///repointed (ancestry-authoritative; skipped at the root), and the node's
    ///children's stored parent fields follow (STORES_PARENTS). returns the
    ///vacated slot. the BLOCK ROOT is not updated — tree-level callers that move
    ///the root do it themselves (`HasRoot`).
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
    ///walk to `sug`'s anchor: (anchor phys, open side, levels back to the current
    ///node). the walker is left AT the anchor — pair with `back_from_anchor`.
    fn walk_to_anchor(&mut self, sug: Suggested) -> (usize, bool, usize);
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
    B::BlockData: HasRoot<B::P>,
{
    ///place `node` as a new child of the current node, routed by `k` (the slot comes
    ///from `lookup`: `Less` ⇒ slot `pos`, `Equal`/`Greater` ⇒ slot `pos+1`; the caller
    ///guarantees the current node is a child-accepting internal — the consumer's
    ///`search` stops at terminals): `suggest_insertion` → walk to the anchor →
    ///`find_slot` → grow fixups → `apply_slide` → `alloc` → ascend
    ///→ node-level wire (`payload` = the caller's promotion data, passed through).
    ///in-order: a LEFT insert (slot < DEGREE/2) shifts the boundary identity and the
    ///parent hops afterward (same rule as a left split; below DEGREE/2 children the
    ///node sits after-all and absorbs). the walker ends at the parent, post-everything.
    ///the hop's `BlockExhausted` leaves the tree position-invalid — the block is
    ///genuinely full at that point; the arena tier's cleave-before-hop is future work.
    fn insert_child(
        &mut self,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        node: B::N,
    ) -> Result<B::P, InsertErr>;
    ///remove child `child_idx` of the current node: node-level unwire + block slot free.
    ///returns the removed node and its freed slot. no slide involved — no fixups.
    fn remove_child(&mut self, child_idx: usize) -> (B::N, OpenSlot);
}

///layer 3 — splits (place-then-split driver: no clone, no placeholder — the
///split node drains its right half straight into a reserved block place, after
///every slide, so no orphan is ever unreached by a fixup walk).
pub trait SplitTreeWalker<'block, NW, B>:
    TreeWalkMut<'block, NW, B> + SplitWalkHelper<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
{
    ///split child `child_idx` of the current node in two; wire the new half in at
    ///`child_idx+1`. ends standing on the parent. `NodeFull` = the parent's child
    ///array is full (caller splits a level up first); `BlockExhausted` = no slot /
    ///spread / edge room (caller cleaves the block).
    fn split_child(&mut self, child_idx: usize) -> Result<(), InsertErr>;
    ///split the root: insert a fresh root above it (per-ordering slot — pre/post
    ///swap it into the old root's phys so the root pointer and vaddr stay valid;
    ///in-order repoints). bumps the block's height. returns the old-root→new-root
    ///address remap (`SwapFixup`) for external vaddr holders (arena parents) —
    ///block data and this walker are already fixed. ends standing on the new root.
    fn split_root(&mut self) -> Result<SwapFixup, InsertErr>;
}

///split machinery — declared here (an inherent impl on the wrapper can't name `B`),
///implemented once for the wrapper where `O`/`B` are both in scope and `self.nw` is
///reachable. not consumer surface. the split machinery binds `Node<P = B::P>`.
///slot-opening/`apply_slide`/`hop_current` live on `TreeWalkHelper` (insert machinery
///the splits borrow).
pub trait SplitWalkHelper<'block, NW, B>: TreeWalkMut<'block, NW, B>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
{
    ///open the split's target slot at `suggest_split`'s anchor; ends standing on the
    ///split node.
    fn open_split_slot(&mut self) -> Result<OpenSlot, InsertErr>;
    ///open two slots at `sug_a`/`sug_b`'s anchors with independent slides
    ///(`find_2_slots`, composed as one `TwoSlide` fixup): both anchors are walked
    ///and both slides computed pre-mutation (the no-outstanding-reservations rule,
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
    fn split_child_here(&mut self, child_idx: usize) -> Result<(), InsertErr>;
    ///Y = `open`: drain the CURRENT node's right half into it, wire at
    ///`child_idx+1` in the parent one level up. ends on the parent.
    fn split_into_open(&mut self, child_idx: usize, open: OpenSlot) -> Result<(), InsertErr>;
    ///place a fresh root above the old one (which the walker stands on). NOT a move
    ///of an existing node: `new_root` is placed and the old root demotes to its
    ///pre-wired child 0. bumps height. where `open` ends up is per-ordering:
    ///in-order — NR takes `open`, R keeps its slot; repoint the root pointer and
    ///step the walker to NR. pre/post — NR is written at `open`, then swapped onto
    ///R's old phys (root pointer and vaddr untouched, walker follows), so R lands
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
    fn prev<'b>(&'b mut self) -> Option<&'b B::N>
    where 'block: 'b {
        let (_, idx) = self.nw.parent()?;
        self.nw.ascend();
        if idx > 0 {
            self.nw.descend(idx - 1);
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

    ///Y lands before `child[mid]` (subtree-first); X keeps its slot. a childless
    ///X (leaf) has no subtree — Y lands right after it.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { before: false };
        }
        Suggested::Child { idx: cc >> 1, before: true }
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
            self.nw.descend(b);
            leftmost_leaf(&mut self.nw);
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            let cc = self.nw.child_count();
            let b = in_boundary::<B>(cc);
            if idx + 1 == b {
                return Some(self.nw.current()); //the parent follows child[b-1]
            }
            if idx + 1 < cc {
                self.nw.descend(idx + 1);
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
            self.nw.descend(b - 1);
            let _ = self.subtree_last();
            return Some(self.nw.current());
        }
        loop {
            let (_, idx) = self.nw.parent()?;
            self.nw.ascend();
            let cc = self.nw.child_count();
            let b = in_boundary::<B>(cc);
            if idx == b {
                return Some(self.nw.current()); //the parent precedes child[b]
            }
            if idx > 0 {
                self.nw.descend(idx - 1);
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
            self.nw.descend(cc - 1);
            levels += 1;
        }
        levels
    }

    ///the node sits in the gap at `b` — a gap AT `b` is the parent's own, so anchor
    ///here (no walk). the side: boundary grows (cc < DEGREE/2, node stays after-all)
    ///⇒ the new subtree lands before the node; boundary fixed (cc ≥ DEGREE/2) ⇒ after.
    ///other gaps → the adjacent child's edge leaf.
    fn suggest_insertion(&self, child_idx: usize) -> Suggested {
        let cc = self.nw.child_count();
        let b = in_boundary::<B>(cc);
        debug_assert!(child_idx <= cc, "suggest_insertion: child_idx out of range");
        if child_idx == b {
            Suggested::Parent { before: cc < <B::N as Node>::DEGREE / 2 }
        } else if child_idx < b {
            Suggested::Child { idx: child_idx, before: true }
        } else {
            Suggested::Child { idx: child_idx - 1, before: false }
        }
    }

    ///Y lands after the rightmost child's subtree (the region end — X, keeping
    ///[0, mid), sits at its own boundary and never moves). a childless X (leaf)
    ///ends its region at itself.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { before: false };
        }
        Suggested::Child { idx: cc - 1, before: false }
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
        self.nw.ascend();
        if idx + 1 < self.nw.child_count() {
            self.nw.descend(idx + 1);
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

    ///X relocates to before `child[mid]` (its kept-half region's end); Y inherits
    ///X's vacated slot. a childless X (leaf) keeps its slot — Y lands right after it.
    fn suggest_split(&self) -> Suggested {
        let cc = self.nw.child_count();
        if cc == 0 {
            return Suggested::Parent { before: false };
        }
        Suggested::Child { idx: cc >> 1, before: true }
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
    B::BlockData: HasRoot<B::P>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn insert_child(
        &mut self,
        k: &<B::N as Node>::K,
        payload: <B::N as Node>::Payload,
        node: B::N,
    ) -> Result<B::P, InsertErr> {
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
        let open = self.apply_slide(&ns, false);
        //place + wire
        self.nw.block_mut().alloc(open).write(node);
        let new_v = self.nw.block().p2v(open.0);
        for _ in 0..levels {
            self.nw.ascend(); //walker: anchor -> back to the current node
        }
        //the new node's own parent field + its (none yet) children (gated)
        self.adopt_node(open.0, self.nw.block().p2v(self.nw.position()));
        self.nw.insert_child(child_idx, k, payload, new_v);
        if O::ORDER == Order::In {
            //a LEFT insert (slot < DEGREE/2) shifted the boundary identity — the
            //parent (current) hops, same rule as a left split; below DEGREE/2
            //children it sits after-all and absorbs.
            let d2 = <B::N as Node>::DEGREE / 2;
            if child_idx < d2 && self.nw.child_count() > d2 {
                self.hop_current()?;
            }
        }
        Ok(new_v)
    }

    fn remove_child(&mut self, child_idx: usize) -> (B::N, OpenSlot) {
        let phys = self.nw.block().v2p(self.nw.child(child_idx));
        let _separator = self.nw.remove_child(child_idx);
        self.nw.block_mut().free(phys)
    }
}

impl<'block, O, NW, B> TreeWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::P>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn reparent_children(&mut self, new_v: B::P) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        for idx in 0..self.nw.child_count() {
            self.nw.descend(idx); //walker: -> child idx
            self.nw.set_parent(new_v);
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
            (lo + 1..=hi).collect::<Vec<_>>()
        } else {
            (lo..hi).collect::<Vec<_>>()
        };
        let back = self.nw.position();
        for q in members {
            self.nw.set_position(q); //walker: -> the moved member (no tree meaning)
            let v = self.nw.block().p2v(q);
            self.reparent_children(v);
        }
        self.nw.set_position(back); //walker: restored
    }

    fn adopt_node(&mut self, p: usize, parent_v: B::P) {
        if !<B::N as Node>::STORES_PARENTS {
            return;
        }
        let back = self.nw.position();
        self.nw.set_position(p); //walker: -> the fresh/moved node (not yet wired)
        self.nw.set_parent(parent_v);
        self.reparent_children(self.nw.block().p2v(p));
        self.nw.set_position(back); //walker: restored
    }

    fn swap_current(&mut self, open: OpenSlot) -> OpenSlot {
        let from = self.nw.position();
        let (freed, to) = self.nw.block_mut().swap_open(from, open);
        let sf = SwapFixup { from, to };
        let (state, block) = self.nw.parts();
        let tr = block.translator();
        state.fixup(&sf, tr);
        if let Some((_, idx)) = self.nw.parent() {
            self.nw.set_child(1, idx, self.nw.block().p2v(to));
        }
        self.reparent_children(self.nw.block().p2v(to));
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
        let steps = hi - lo;
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
            //it are exactly its members. a reserved-but-unwired Some (alloc
            //without write/wire) displaces a visit outside the range — the
            //tripwire for what would otherwise surface later as assume_init UB
            //(subtle_bugs.md §6).
            assert!(
                lo <= p && p <= hi,
                "fixup: run walk left the run — an occupied slot in the run is \
                 unwired (alloc without write/wire?)"
            );
            if let Some((pphys, idx)) = self.nw.parent() {
                //parent moved iff its phys is inside the closed run (it can't be `from`
                //— that's the None slot).
                let parent_moved = pphys != ns.from && lo <= pphys && pphys <= hi;
                let pv = if parent_moved { pphys.wrapping_add(delta as usize) } else { pphys };
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
             unwired (alloc without write/wire?), or the walk was entered mid-run"
        );
        //position-neutral: back at the anchor with entry ancestry — no walking.
        *self.nw.parts().0 = snapshot;
    }

    fn apply_slide(&mut self, ns: &NoneSlide, far_short: bool) -> OpenSlot {
        self.fixup(ns, far_short);
        let open = self.nw.block_mut().slide_none(*ns);
        let (state, block) = self.nw.parts();
        let tr = block.translator();
        state.fixup(ns, tr);
        self.reparent_run(ns);
        open
    }

    fn walk_to_anchor(&mut self, sug: Suggested) -> (usize, bool, usize) {
        match sug {
            Suggested::Child { idx, before } => {
                self.nw.descend(idx);
                let e = if before { self.subtree_first() } else { self.subtree_last() };
                (self.nw.position(), !before, 1 + e)
            }
            Suggested::Parent { before } => (self.nw.position(), !before, 0),
        }
    }

    fn back_from_anchor(&mut self, levels: usize) {
        for _ in 0..levels {
            self.nw.ascend();
        }
    }

    ///open a slot adjacent-after the current node. right-side None: hole at
    /// pos+1, the node stays. left-side fallback (to = pos): the node itself
    /// shifts left one and the hole opens at its old slot — still adjacent-after
    /// the (moved) node; the walker state follows either way. ends standing on
    /// the current node.
    fn open_after(&mut self) -> Result<OpenSlot, InsertErr> {
        let pos = self.nw.position();
        let found = self.nw.block_mut().find_slot(pos, true);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            state.fixup(g, block.translator());
        }
        let Some(ns) = found.slide else {
            return Err(InsertErr::BlockExhausted);
        };
        Ok(self.apply_slide(&ns, false))
    }

    fn open_suggested(&mut self, sug: Suggested) -> Result<OpenSlot, InsertErr> {
        let (anchor, after, levels) = self.walk_to_anchor(sug);
        let found = self.nw.block_mut().find_slot(anchor, after);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            state.fixup(g, block.translator());
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
        //hoppee is mid-hop (physically at its old gap, `in_boundary` already
        //claims the new one), so WHERE the None lands picks the walk's anchor:
        let (anchor, _, levels) =
            self.walk_to_anchor(Suggested::Child { idx: b - 1, before: false });
        let found = self.nw.block_mut().find_slot(anchor, true);
        if let Some(g) = found.grew.as_ref() {
            let (state, block) = self.nw.parts();
            let tr = block.translator();
            state.fixup(g, tr);
            g.fix_p(&mut hoppee);
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
            let far_short = ns.from == hoppee + 1 && ns.from - ns.to > 1;
            let open = self.apply_slide(&ns, far_short);
            self.back_from_anchor(levels);
            open
        } else {
            //None between the gap and the hoppee: the run is entirely consistent
            //child[b]-side nodes — walk it from its first member
            //(subtree_first(child[b]) sits exactly at the slide's `to`), so the
            //walk never crosses the misplaced hoppee.
            self.back_from_anchor(levels);
            let (anchor_r, after_r, levels_r) =
                self.walk_to_anchor(Suggested::Child { idx: b, before: true });
            debug_assert!(!after_r && anchor_r == ns.to, "hop: child[b] edge != slide to");
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
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
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
            state.fixup(g, block.translator());
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
    fn split_child_here(&mut self, child_idx: usize) -> Result<(), InsertErr> {
        self.nw.descend(child_idx);
        let open = self.open_split_slot()?;
        if O::ORDER == Order::Post && self.nw.child_count() > 0 {
            //X (current) relocates into the opened slot via swap_current (state
            //follows, parent entry repointed, children reparented); Y (the drained
            //right half) inherits X's vacated slot.
            let freed = self.swap_current(open);
            let y_v = self.nw.block().p2v(freed.0);
            let x_phys = self.nw.position();
            let (x, cell) = self.nw.block_mut().alloc_disjoint_mut(x_phys, freed);
            let (sep, payload) = x.split(cell);
            //Y's drained children name X — adopt Y under X's parent (gated)
            if let Some((pp, _)) = self.nw.parent() {
                self.adopt_node(freed.0, self.nw.block().p2v(pp));
            }
            self.nw.ascend(); //walker: X (the split node) -> its tree parent
            self.nw.insert_child(child_idx + 1, &sep, payload, y_v);
            Ok(())
        } else {
            //Y = the opened slot; X untouched (preorder: X keeps its slot;
            //in-order: X sits at its boundary — `in_boundary`; postorder leaf:
            //X's region is just itself, right of which Y lands).
            self.split_into_open(child_idx, open)
        }
    }

    fn split_into_open(&mut self, child_idx: usize, open: OpenSlot) -> Result<(), InsertErr> {
        let x = self.nw.position();
        let y_v = self.nw.block().p2v(open.0);
        let (x, cell) = self.nw.block_mut().alloc_disjoint_mut(x, open);
        let (sep, payload) = x.split(cell);
        //Y's drained children name X — adopt Y under the tree parent (gated)
        if let Some((pp, _)) = self.nw.parent() {
            self.adopt_node(open.0, self.nw.block().p2v(pp));
        }
        self.nw.ascend(); //walker: X (the split node) -> its tree parent
        self.nw.insert_child(child_idx + 1, &sep, payload, y_v);
        Ok(())
    }

    /// in order places the new root at open, pre and post put the old root at open and the new root takes its place.
    fn promote_new_root(&mut self, open: OpenSlot) {
        if O::ORDER == Order::In {
            let r_phys = self.nw.position();
            let r_v = self.nw.block().p2v(r_phys);
            self.nw.block_mut().alloc(open).write(<B::N as SplittableNode>::new_root(r_v));
            let d = self.nw.block_mut().data_mut();
            d.set_root(open.0);
            d.set_height(d.height() + 1);
            self.nw.set_position(open.0); //walker steps to NR (R unreachable through children yet)
            //R keeps its slot but demotes under NR — its parent field names NR
            //(children unchanged; the reparent is idempotent) (gated)
            self.adopt_node(r_phys, self.nw.block().p2v(open.0));
        } else {
            //child 0 = the old root's POST-swap vaddr (it lands at `open`)
            let r_phys = self.nw.position();
            let r_v = self.nw.block().p2v(open.0);
            self.nw.block_mut().alloc(open).write(<B::N as SplittableNode>::new_root(r_v));
            //raw swap: NR lands at r_phys — the walker's position now names NR
            //(not R); R is at `open`
            self.nw.block_mut().swap(open.0, r_phys);
            let d = self.nw.block_mut().data_mut();
            d.set_height(d.height() + 1);
            //R moved to `open` and demoted under NR: its parent field names NR
            //and its children follow it (gated)
            self.adopt_node(open.0, self.nw.block().p2v(r_phys));
        }
    }
}

impl<'block, O, NW, B> SplitTreeWalker<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B>,
{
    fn split_child(&mut self, child_idx: usize) -> Result<(), InsertErr> {
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
        let r_phys = self.nw.position();
        match O::ORDER {
            //NR takes R's slot (root-first), R lands right of it; the swap keeps
            //the walker on the root and the root vaddr stable (no-op remap).
            Order::Pre => {
                let open = self.open_after()?;
                self.promote_new_root(open);
                self.split_child_here(0)?;
                Ok(SwapFixup::no_op(r_phys))
            }
            //root-last: NR ends up after everything. INTERNAL R: both slots open
            //up front via find_2_slots (independent slides) while the tree is
            //fully consistent — r_slot at R's kept-half region end (before
            //subtree(mid)), y_slot after child[cc-1] — then drain R into y_slot,
            //then NR into r_slot + swap with R: NR lands under the walker at
            //r_phys, R at r_slot (its post-split position). no walk runs in the
            //transient window between drain and swap (subtle_bugs.md §1).
            //LEAF R: childless — nothing to relocate; Y right after R, NR right
            //after Y; each slot is written before the next opens, so plain
            //sequential opens suffice (and R was the last node, so the run right
            //of Y is empty — the fixup walk is a no-op where Y is still unwired).
            Order::Post => {
                let cc = self.nw.child_count();
                if cc == 0 {
                    let y_open = self.open_split_slot()?; //Parent{after} — right of R
                    let (sep, payload) = {
                        let (x, cell) = self.nw.block_mut().alloc_disjoint_mut(r_phys, y_open);
                        x.split(cell)
                    };
                    let found = self.nw.block_mut().find_slot(y_open.0, true);
                    let (mut y_open, mut r_phys) = (y_open, r_phys);
                    if let Some(g) = found.grew.as_ref() {
                        let (state, block) = self.nw.parts();
                        state.fixup(g, block.translator());
                        //the grow moved Y (written) and possibly R — both are
                        //live phys held across this find_slot and must follow
                        g.fix_p(&mut y_open.0);
                        g.fix_p(&mut r_phys);
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
                    let nr = <B::N as SplittableNode>::new_root(self.nw.block().p2v(r_phys));
                    self.nw.block_mut().alloc(nr_open).write(nr);
                    {
                        let d = self.nw.block_mut().data_mut();
                        d.set_root(nr_open.0);
                        d.set_height(d.height() + 1);
                    }
                    self.nw.set_position(nr_open.0); //postorder's one bend (leaf root)
                    //R (leaf, unmoved) and Y (fresh) both demote under NR (gated —
                    //leaf R has no children; Y's field is the only work)
                    self.adopt_node(r_phys, self.nw.block().p2v(nr_open.0));
                    self.adopt_node(y_open.0, self.nw.block().p2v(nr_open.0));
                    self.nw.insert_child(1, &sep, payload, self.nw.block().p2v(y_open.0));
                    //the root vaddr moves — a real (non no-op) remap
                    Ok(SwapFixup { from: r_phys, to: nr_open.0 })
                } else {
                    let (r_slot, y_slot) = self.open_two(
                        self.suggest_split(),
                        Suggested::Child { idx: cc - 1, before: false },
                    )?;
                    //R may have moved with open_two's slides (the walker followed
                    //it) — the pre-open r_phys is stale; reread
                    let r_phys = self.nw.position();
                    let (sep, payload) = {
                        let (x, cell) = self.nw.block_mut().alloc_disjoint_mut(r_phys, y_slot);
                        x.split(cell)
                    };
                    let nr = <B::N as SplittableNode>::new_root(self.nw.block().p2v(r_slot.0));
                    self.nw.block_mut().alloc(r_slot).write(nr);
                    //raw swap: NR lands at r_phys — the walker's position now
                    //names NR (not R); R is at r_slot
                    self.nw.block_mut().swap(r_slot.0, r_phys);
                    let d = self.nw.block_mut().data_mut();
                    d.set_height(d.height() + 1);
                    //R moved to r_slot, Y fresh at y_slot — both demote under NR,
                    //Y's drained children name R (gated)
                    self.adopt_node(r_slot.0, self.nw.block().p2v(r_phys));
                    self.adopt_node(y_slot.0, self.nw.block().p2v(r_phys));
                    self.nw.insert_child(1, &sep, payload, self.nw.block().p2v(y_slot.0));
                    Ok(SwapFixup::no_op(r_phys))
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
                        idx:    self.nw.child_count() - 1,
                        before: false,
                    })?
                };
                self.promote_new_root(open);
                self.split_child_here(0)?;
                //the root vaddr moves — a real (non no-op) remap. `to` from the
                //walker's live position: the child-split's slide may have moved
                //NR off `open` (state + block data are fixed; `open.0` is stale)
                Ok(SwapFixup { from: r_phys, to: self.nw.position() })
            }
        }
    }
}

// ---- shared walk helpers (free fns over the consumer walker) ----

///in-order position boundary: the node sits between child[b-1] and child[b],
///`b = min(cc, DEGREE/2)` — after all children when cc ≤ DEGREE/2 (fixed by DEGREE,
///not cc: a full node's boundary is exactly its kept-left-half's edge, so splits
///never move the split node).
fn in_boundary<'block, B: BlockTrait<'block>>(cc: usize) -> usize
where B::N: Node {
    cc.min(<B::N as Node>::DEGREE / 2)
}

fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
{
    while nw.parent().is_some() {
        nw.ascend();
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
        nw.descend(0);
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
        nw.descend(nw.child_count() - 1);
        levels += 1;
    }
    levels
}

#[cfg(test)]
#[path = "tests/walker.rs"]
mod tests;
