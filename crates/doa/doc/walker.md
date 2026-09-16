```rust
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
///L0025
///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}
///L0033
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
///L0044
///`TreeWalkMut::insert_child` failure modes. either we're out of memory in the node or we're out 
/// of addresses/memory in the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertErr {
    ///the current node has no room for another child (split the node).
    NodeFull,
    ///the block is exhausted — no slot, no spread, no edge room (split the block).
    BlockExhausted,
}
///L0051
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
///L0073
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
///L0096
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
    fn children(&self) -> impl Iterator<Item = B::A> + '_;
    ///node-level relative position of `k` among the current node's ordered children:
    ///`(child slot, cmp)`. search owns routing.
    fn lookup(&self, k: &<B::N as Node>::K) -> (ChildPos, Ordering);
    ///position of the current node.
    fn position(&self) -> Pos;
    fn current<'b>(&'b self) -> &'b B::N
    where 'block: 'b;
    ///descend into child `child` — child → `a2p` → the state's descent
    ///record (no-op for a stackless state) + reposition. the walker names the
    ///child afterward.
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b B::N
    where 'block: 'b;
    ///descend from the current node using k to lookup a child of the current node repeatedly.
    fn search(&mut self, k: &<B::N as Node>::K) -> Option<&B::N>;
}
///L0151
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
///L0165
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
    where 'block: 'b;
    ///current node has room for one more child/payload.
    fn has_space(&self) -> bool;
    ///reposition the walker: it names the node at `pos` (no tree meaning — state only).
    fn set_position(&mut self, pos: Pos);
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
///L0214
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
///L0251
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
///L0317
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
///L0348
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
///L0375
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
    fn split_into_open(&mut self, child_idx: ChildPos, open: OpenSlot) -> Result<(), InsertErr>;
    ///place a fresh root above the old one (which the walker stands on). NOT a move
    ///of an existing node: `new_root` is placed and the old root demotes to its
    ///pre-wired child 0. bumps height. where `open` ends up is per-ordering:
    ///in-order — NR takes `open`, R keeps its slot; repoint the root pointer and
    ///step the walker to NR. pre/post — NR is written at `open`, then swapped onto
    ///R's old position (root pointer and addr untouched, walker follows), so R lands
    ///at `open`. ends standing on NR.
    fn promote_new_root(&mut self, open: OpenSlot);
}
///L0415
impl<O, NW> TreeWalker<O, NW> {}
///L0421
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block,
    B::N: Node {}
///L0509
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block,
    B::N: Node {}
///L0643
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PostOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PostOrder> + 'block,
    B::N: Node {}
///L0740
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
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L0811
impl<'block, O, NW, B> TreeWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L1064
impl<'block, O, NW, B> SplitWalkHelper<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L1183
impl<'block, O, NW, B> SplitTreeWalker<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<A = B::A>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
// ---- shared walk helpers (free fns over the consumer walker) ----
///L1332
///in-order position boundary: the node sits between child[b-1] and child[b],
///`b = min(cc, DEGREE/2)` — after all children when cc ≤ DEGREE/2 (fixed by DEGREE,
///not cc: a full node's boundary is exactly its kept-left-half's edge, so splits
///never move the split node).
fn in_boundary<'block, B: BlockTrait<'block>>(cc: usize) -> ChildPos
where B::N: Node;
///L1337
fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L1348
fn leftmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L1362
fn rightmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
//tests unwired for the addr/pos terminology refactor (Pos/ChildPos/Rel signatures)
//— port src/tests/walker.rs to the new surface, then re-enable:
//#[cfg(test)]
//#[path = "tests/walker.rs"]
//mod tests;
```
