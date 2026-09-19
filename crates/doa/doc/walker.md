```rust
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
///L0026
///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}
///L0031
pub type PreOrderWalker<NW> = TreeWalker<PreOrder, NW>;
///L0032
pub type InOrderWalker<NW> = TreeWalker<InOrder, NW>;
///L0037
///the block is exhausted — no slot, no spread, no edge room. split the block.
///the walker was consumed: rebuild from the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockExhausted;
///L0044
///what an open applied: the grow remap + the slide pair (`b` is a no-op slide
/// for single opens). consumers holding addresses/positions across the open
/// apply these via their `Fixable` impl — held addrs need only the slides
/// (addrs are grow-stable by construction), held positions need both.
#[derive(Clone, Copy)]
pub struct OpenFixups {
    pub grew:   Option<GrewFixup>,
    pub slides: DoubleSlide,
}
///L0049
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
///L0070
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
    where 'block: 'b;
    ///descend by `k` until `lookup` returns None. the terminal node.
    fn search<'b>(&'b mut self, k: &<B::N as Node>::K) -> Option<&'b B::N>
    where 'block: 'b;
}
///L0114
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
///L0129
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
///L0160
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
///L0185
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
///L0218
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
///L0246
///(internal) where an open anchors: the current node's own position, or a
/// child's subtree edge.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    Here { rel: Rel },
    Child { idx: ChildPos, rel: Rel },
}
///L0255
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
///L0275
impl<O, NW> TreeWalker<O, NW> {}
///L0281
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block,
    B::N: Node {}
///L0345
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block,
    B::N: Node {}
///L0453
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
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L0661
impl<'block, NW, B> PreOrderWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block + BlockOps<'block>,
    B::N: Node,
    TreeWalker<PreOrder, NW>: TreeWalk<'block, NW, B>,
    TreeWalker<PreOrder, NW>: TreeWalkHelper<'block, NW, B> {}
///L0701
impl<'block, NW, B> InOrderWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block + BlockOps<'block>,
    B::N: Node,
    B::BlockData: HasRoot<B::A>,
    TreeWalker<InOrder, NW>: TreeWalk<'block, NW, B>,
    TreeWalker<InOrder, NW>: TreeWalkHelper<'block, NW, B> {}
// ---- shared walk helpers (free fns over the consumer walker) ----
///L0846
///in-order position boundary: the node sits between child[b-1] and child[b],
///`b = min(cc, DEGREE/2)` — after all children when cc ≤ DEGREE/2 (fixed by DEGREE,
///not cc: a full node's boundary is exactly its kept-left-half's edge, so splits
///never move the split node).
fn in_boundary<'block, B: BlockTrait<'block>>(cc: usize) -> ChildPos
where B::N: Node;
///L0851
fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L0862
fn leftmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L0876
fn rightmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L0892
#[cfg(test)]
#[path = "tests/walker.rs"]
mod tests;
```
