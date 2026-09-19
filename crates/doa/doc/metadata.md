```rust
//!the fixup protocol: block ops hand back fixup structs (`GrewFixup`, `SwapFixup`,
//!`NoneSlide`, `DoubleSlide`, `GatherSlide`), each impling `Fixup` — the uniform
//!contract
//!(`affects_pos`/`fix_pos` direct; `affects_addr`/`fix_addr` through the block's
//!translator). tracked state (block data, walker state) impls `Fixable` — one
//!method per fixup kind, called unconditionally by the walker with the block's
//!translator; the impl decides relevance. also the cursor/walker/block data
//!types (`Pos`, `ChildPos`, `PosAncestry`, `Root`, `Ancestry`, `CursorState`, ...).
///L0022
///spread remap `pos → pos<<shl + shift_offset` (grow doubles the store; addrs
///stay stable). `{shl: 0, shift_offset: 1}` doubles as the plain `pos → pos+1`
///(Pluripotent front-edge grow).
#[derive(Clone, Copy)]
pub struct GrewFixup {
    pub shl:          u32,
    pub shift_offset: u8,
}
///L0032
///a swap exchanged the record at `from` with the None at `to`. only the moved
///record's position remaps (from → to). swaps emit no self-fixup — the mover applies
///this by hand to block data + walker state, and `split_root` returns it as the
///old-root→new-root remap for external addr holders (arena parents) to apply.
#[derive(Clone, Copy, Debug)]
pub struct SwapFixup {
    pub from: Pos,
    pub to:   Pos,
}
///L0043
///two non-overlapping slides from one `find_2_slots` — the address fixup for a
///two-slot reservation, so holders get ONE `fixup` call covering both (order-
///independent: disjoint runs). the applying side still slides them separately
///(the run-parent fixups interleave with the slides and cannot compose;
///subtle_bugs.md §9).
#[derive(Clone, Copy, Debug)]
pub struct DoubleSlide {
    pub a: NoneSlide,
    pub b: NoneSlide,
}
///L0055
///one N-None gather: the `holes.len()` Nones at `holes` (sorted, all on
///`rel`'s side of `anchor`) compact contiguously beside the anchor; every
///intervening Some crosses the None-run exactly once (swap-minimal — a
///stable compaction, one move per member). the per-element remap is a
///crossing count, NOT sequential `NoneSlide`s — the runs overlap, so
///coordinates go stale mid-application.
#[derive(Clone, Debug)]
pub struct GatherSlide {
    pub anchor: Pos,
    pub rel:    Rel,
    pub holes:  Vec<Pos>,
}
///L0061
impl GatherSlide {}
///L0098
impl Fixup for GatherSlide {}
///L0121
///a slot in the store's array — the truth: pos 0 holds the min element, pos
///len−1 the max; addr→pos is the translator's job. also the stackless cursor
///state: descends freely (no per-level record), `ascend`/`parent` report
///nothing. derived `Ord` + usize-rhs `+`/`-` keep inline arithmetic bare;
///`.0` for the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos(pub usize);
///L0126
///a child's slot within its parent's child sequence; as an insertion gap it
///may equal `child_count`. compares against usize (`idx + 1 < child_count()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChildPos(pub usize);
///L0131
///tree height for fixed-height trees (b+ / S trees). pointer-free no-op-fixable
///level counter — a component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);
///L0136
///walker's current depth. pointer-free no-op-fixable level counter — a
///component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);
///L0141
///minimal tree block data: root position + tree height (Fixable + HasRoot). not a
///walker state — a `reposition` on block data would rewrite the block root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    root:   Pos,
    height: u32,
}
///L0148
///one ancestor entry: parent node's position + the child slot we descended through.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ancestor {
    pub parent: Pos,
    pub child:  ChildPos,
}
///L0154
///inline backing depth — deeper paths spill to the heap.
pub const ANCESTRY_INLINE: usize = 8;
///L0162
///stackful walker's ancestor stack, one entry per level. stores pos (not addr): fixup
///applies `fix_pos` directly, no translator; O(height) per op. inline-backed
///(`ANCESTRY_INLINE` entries, heap spill beyond): fixup's per-slide snapshot/
///restore clones a fixed-size block — no heap below that depth.
///todo : optimization : ancestry is sorted for preorder and postorder, those shouldnt have to check every item every time.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    inline: [Ancestor; ANCESTRY_INLINE],
    len:    usize,
    spill:  Vec<Ancestor>,
}
///L0172
///pos + ancestry — the standard stackful walker state: satisfies the
///`NodeCursor::State` (`CursorState`) contract for any stackful walker, so
///consumers embed it instead of reimplementing the fixup loop.
#[derive(Clone, Debug, Default)]
pub struct PosAncestry {
    pub pos:      Pos,
    pub ancestry: Ancestry,
}
///L0183
///one block op's address remap — the contract every fixup struct satisfies.
///`affects_*` guards its `fix_*`. pos-space is direct; addr-space goes through
///the block's translator — addrs are slot-derived names, so knob-free ops
///(slide/swap) remap them via the unchanged translator, while grow/spread turn
///translator knobs and leave addrs stable: `affects_addr` false, `fix_addr`
///no-op.
pub trait Fixup {
    fn affects_pos(&self, pos: Pos) -> bool;
    ///raw remap — precondition `affects_pos`.
    fn fix_pos(&self, pos: &mut Pos);
    fn affects_addr<A: Addr>(&self, addr: A, tr: &Translator<A>) -> bool;
    ///raw remap — precondition `affects_addr`.
    fn fix_addr<A: Addr>(&self, addr: &mut A, tr: &Translator<A>);
}
///L0197
///tracked tree state (block data, walker data) holding addresses. the walker calls
///the matching method unconditionally after every op that produces that fixup kind,
///passing the block's post-op translator — the IMPL decides relevance: pos-holders
///guard with `affects_pos`/`fix_pos` and ignore the translator, addr-holders go
///through `affects_addr`/`fix_addr`.
pub trait Fixable<A: Addr> {
    fn grew_fix(&mut self, fix: GrewFixup, tr: &Translator<A>);
    fn swap_fix(&mut self, fix: SwapFixup, tr: &Translator<A>);
    fn slide_fix(&mut self, fix: NoneSlide, tr: &Translator<A>);
    fn two_slide(&mut self, fix: DoubleSlide, tr: &Translator<A>);
    ///by-ref: the gather plan is not `Copy` (its hole list scales with n).
    fn gather_fix(&mut self, fix: &GatherSlide, tr: &Translator<A>);
}
///L0211
///walker state seam: the crate's defaults (position/current/descend) run on it —
///`Fixable` via supertrait, so every grow/slide/swap fixup corrects the state;
///`Clone` because the run-walk fixup snapshots + restores it.
///PER IMPLEMENTOR: a stackless cursor picks `Pos`, a stackful walker picks
///`PosAncestry`.
pub trait CursorState<A: Addr>: Fixable<A> + Clone {
    fn position(&self) -> Pos;
    fn reposition(&mut self, pos: Pos);
    ///record one descent (no-op for a stackless state).
    fn descend(&mut self, parent: Pos, child: ChildPos);
}
///L0220
///block data that exposes a movable root position + the tree height (splits' root
///promotion bumps it; the consumer's `is_leaf` reads it). extends `Fixable`.
pub trait HasRoot<A: Addr>: Fixable<A> {
    fn root(&self) -> Pos;
    fn set_root(&mut self, root: Pos);
    fn height(&self) -> u32;
    fn set_height(&mut self, height: u32);
}
///L0227
impl Pos {}
///L0239
impl Add<usize> for Pos {}
///L0247
impl Sub<usize> for Pos {}
///L0255
impl Add<usize> for ChildPos {}
///L0263
impl Sub<usize> for ChildPos {}
///L0271
impl PartialEq<usize> for ChildPos {}
///L0278
impl PartialOrd<usize> for ChildPos {}
///L0286
///whole-store remap — every position moves.
impl Fixup for GrewFixup {}
///L0303
///only the run between `from` and `to` shifts; the gap slot at `from` is
///vacated, not moved. `from == to` ⇒ nothing moves.
impl Fixup for NoneSlide {}
///L0324
impl SwapFixup {}
///L0333
///only the moved record remaps.
impl Fixup for SwapFixup {}
///L0349
///affected by either run.
impl Fixup for DoubleSlide {}
///L0373
impl Ancestry {}
///L0405
impl Index<usize> for Ancestry {}
///L0413
impl IndexMut<usize> for Ancestry {}
///L0424
impl<A: Addr> Fixable<A> for Pos {}
///L0451
///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Height {}
///L0460
///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Depth {}
///L0468
impl<A: Addr> Fixable<A> for Root {}
///L0494
impl<A: Addr> Fixable<A> for Ancestry {}
///L0530
impl<A: Addr> Fixable<A> for PosAncestry {}
///L0562
///blanket: pointer-free block data.
impl<A: Addr> Fixable<A> for () {}
///L0570
impl<A: Addr> CursorState<A> for Pos {}
///L0580
impl<A: Addr> CursorState<A> for PosAncestry {}
///L0592
impl<A: Addr> HasRoot<A> for Root {}
```
