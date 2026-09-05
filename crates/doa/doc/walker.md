```rust
//! the node contract (`Node`/`SplittableNode`) + the three walker layers + the
//! split driver. layer 1 — `NodeCursor`/`NodeWalker`/`NodeWalkerMut`: the
//! consumer-implemented mask over the node representation (the crate never sees
//! union/enum/whatever). layer 2 — `TreeWalker<O, NW>` + `TreeWalk`: ordered
//! traversal, one impl per ordering. layer 3 — `TreeWalkMut` + the split
//! machinery: tree ops over the unified `BlockOps` surface. `B` is a trait param
//! at every level; `O` is never a param — it is always `B::O` (the wrapper
//! carries it as phantom data). no `insert_child` name collision:
//! `NodeWalkerMut` is impl'd on the consumer's `NW`, `TreeWalkMut` on the
//! wrapper — different `Self` types, method sets never intersect.
///L0024
///ordering-aware wrapper over any consumer `NW`. `O` is phantom — it tags the wrapper
/// so the per-ordering impls sit on distinct self types (coherence), and is bound to
/// the block's ordering at every use (`B: BlockTrait<O = O>`).
pub struct TreeWalker<O, NW> {
    pub nw: NW,
    _o:     PhantomData<O>,
}
///L0033
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
///L0045
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
///L0052
pub trait Node {
    type K;
    type V;
    type P: BlockIndex;
    ///maximum children per node (in-order layout's parent gap scales with it).
    ///≥ 3 — a full node needs ≥2 keys to split into two non-degenerate halves + a
    ///separator.
    const DEGREE: usize;
    ///do nodes store parent-pointer fields? gates the reparent machinery
    ///(`swap_current`, the NoneSlide fixup, `insert_new_root`); false shapes pay
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
///L0073
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
///L0095
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
    fn children(&self) -> impl Iterator<Item = B::P> + '_;
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
    fn position(&self) -> usize;
    fn current<'b>(&'b self) -> &'b B::N
    where 'block: 'b;
    ///descend into child `child_idx` — child → `v2p` → the state's descent
    ///record (no-op for a stackless state) + reposition.
    fn descend<'b>(&'b mut self, child_idx: usize) -> &'b B::N
    where 'block: 'b;
    ///descend from the current node to `k`'s terminal (None conventionally = empty
    ///block). consumer-implemented routing over `lookup`'s `(pos, cmp)` — equal-right,
    ///Eq-stop, …: baking in a default would fix the meaning of `Equal`, which is a
    ///per-shape decision.
    fn search(&mut self, k: &<B::N as Node>::K) -> Option<&B::N>;
}
///L0162
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
///L0177
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
    where 'block: 'b;
    ///reposition to phys `p` without walking (ancestry untouched — the caller
    ///guarantees consistency; root promotion uses it: the new root is not yet
    ///reachable through children).
    fn set_position(&mut self, p: usize);
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
}
///L0303
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
///L0334
///layer 3 — tree ops, crate-implemented over the unified `BlockOps` surface; the
///per-ordering `suggest_insertion`/subtree edges come in via the `TreeWalk`
///supertrait. `B::BlockData: HasRoot` — the hop may move the block root.
pub trait TreeWalkMut<'block, NW, B>: TreeWalk<'block, NW, B>
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
    ///run-parent-fixup for a pending slide `ns` — BEFORE the slide is applied, rewrite
    ///each moved node's parent→child pointer (and the moved node's stored parent field
    ///when its parent also moved; no-op for parent-free shapes). the walker must be
    ///positioned at the slide's anchor with valid ancestry.
    fn fixup(&mut self, ns: &NoneSlide);
    ///apply a pending slide: run-parent-fixup → `slide_none` → walker-state fixup →
    ///`reparent_run` (STORES_PARENTS). THE chokepoint — every slide in the tree ops
    ///goes through here. returns the opened slot.
    fn apply_slide(&mut self, ns: &NoneSlide) -> OpenSlot;
    ///walk to `sug`'s anchor: (anchor phys, open side, levels back to the current
    ///node). the walker is left AT the anchor — pair with `back_from_anchor`.
    fn walk_to_anchor(&mut self, sug: Suggested) -> (usize, bool, usize);
    ///ascend `levels` — the inverse of `walk_to_anchor`.
    fn back_from_anchor(&mut self, levels: usize);
    ///open a slot adjacent after the CURRENT node (find_slot + grow fixups; the
    ///slide can't move the current node — the run is right of it). ends standing
    ///on the current node.
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
///L0393
///layer 3 — splits (place-then-split driver: no clone, no placeholder — the
///split node drains its right half straight into a reserved block place, after
///every slide, so no orphan is ever unreached by a fixup walk).
pub trait SplitTreeWalker<'block, NW, B>:
    TreeWalkMut<'block, NW, B> + SplitWalkerExt<'block, NW, B>
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
///L0420
///split machinery — declared here (an inherent impl on the wrapper can't name `B`),
///implemented once for the wrapper where `O`/`B` are both in scope and `self.nw` is
///reachable. not consumer surface. the split machinery binds `Node<P = B::P>`.
///slot-opening/`apply_slide`/`hop_current` live on `TreeWalkMut` (insert machinery
///the splits borrow).
pub trait SplitWalkerExt<'block, NW, B>: TreeWalkMut<'block, NW, B>
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
    ///insert a fresh root above the old one (which the walker stands on) into the
    ///opened slot — NOT a move of an existing node: `new_root` is placed and the
    ///old root demotes to its pre-wired child 0. bumps height. pre/post: swap with
    ///the old root — NR takes its phys (under the walker's feet; root pointer and
    ///vaddr untouched, R lands at `open`). in-order: R keeps its slot (no swap puts
    ///NR right of R under the walker's feet) — repoint the root pointer and step
    ///the walker to NR.
    fn insert_new_root(&mut self, open: OpenSlot);
}
///L0460
impl<O, NW> TreeWalker<O, NW> {}
///L0466
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PreOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PreOrder> + 'block,
    B::N: Node {}
///L0554
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<InOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = InOrder> + 'block,
    B::N: Node {}
///L0673
impl<'block, NW, B> TreeWalk<'block, NW, B> for TreeWalker<PostOrder, NW>
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block, O = PostOrder> + 'block,
    B::N: Node {}
///L0769
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
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L0967
impl<'block, O, NW, B> SplitWalkerExt<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
///L1083
impl<'block, O, NW, B> SplitTreeWalker<'block, NW, B> for TreeWalker<O, NW>
where
    O: crate::Ordering,
    NW: NodeWalkerMut<'block, B>,
    B: BlockTrait<'block> + 'block + BlockOps<'block>,
    B::N: Node<P = B::P>,
    B::N: SplittableNode,
    B::BlockData: HasRoot<B::P>,
    TreeWalker<O, NW>: TreeWalk<'block, NW, B> {}
// ---- shared walk helpers (free fns over the consumer walker) ----
///L1222
///in-order position boundary: the node sits between child[b-1] and child[b],
///`b = min(cc, DEGREE/2)` — after all children when cc ≤ DEGREE/2 (fixed by DEGREE,
///not cc: a full node's boundary is exactly its kept-left-half's edge, so splits
///never move the split node).
fn in_boundary<'block, B: BlockTrait<'block>>(cc: usize) -> usize
where B::N: Node;
///L1227
fn at_root<'block, NW, B>(nw: &mut NW)
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L1238
fn leftmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
///L1252
fn rightmost_leaf<'block, NW, B>(nw: &mut NW) -> usize
where
    NW: NodeWalker<'block, B>,
    B: BlockTrait<'block> + 'block,
    B::N: Node,
;
```
