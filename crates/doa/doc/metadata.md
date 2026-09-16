```rust
//!the fixup protocol: block ops hand back fixup structs (`GrewFixup`, `SwapFixup`,
//!`NoneSlide`, `DoubleSlide`), each impling `Fixup` — the uniform contract
//!(`affects_pos`/`fix_pos` direct; `affects_addr`/`fix_addr` through the block's
//!translator). tracked state (block data, walker state) impls `Fixable` — one
//!method per fixup kind, called unconditionally by the walker with the block's
//!translator; the impl decides relevance. also the cursor/walker/block data
//!types (`Pos`, `ChildPos`, `PosAncestry`, `Root`, `Ancestry`, `CursorState`, ...).
///L0019
///spread remap `pos → pos<<shl + shift_offset` (grow doubles the store; addrs
///stay stable). `{shl: 0, shift_offset: 1}` doubles as the plain `pos → pos+1`
///(Pluripotent front-edge grow).
#[derive(Clone, Copy)]
pub struct GrewFixup {
    pub shl:          u32,
    pub shift_offset: u8,
}
///L0029
///a swap exchanged the record at `from` with the None at `to`. only the moved
///record's position remaps (from → to). swaps emit no self-fixup — the mover applies
///this by hand to block data + walker state, and `split_root` returns it as the
///old-root→new-root remap for external addr holders (arena parents) to apply.
#[derive(Clone, Copy, Debug)]
pub struct SwapFixup {
    pub from: Pos,
    pub to:   Pos,
}
///L0040
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
///L0051
///a slot in the store's array — the truth: pos 0 holds the min element, pos
///len−1 the max; addr→pos is the translator's job. also the stackless cursor
///state: descends freely (no per-level record), `ascend`/`parent` report
///nothing. derived `Ord` + usize-rhs `+`/`-` keep inline arithmetic bare;
///`.0` for the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos(pub usize);
///L0056
///a child's slot within its parent's child sequence; as an insertion gap it
///may equal `child_count`. compares against usize (`idx + 1 < child_count()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChildPos(pub usize);
///L0061
///tree height for fixed-height trees (b+ / S trees). pointer-free no-op-fixable
///level counter — a component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);
///L0066
///walker's current depth. pointer-free no-op-fixable level counter — a
///component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);
///L0071
///minimal tree block data: root position + tree height (Fixable + HasRoot). not a
///walker state — a `reposition` on block data would rewrite the block root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    root:   Pos,
    height: u32,
}
///L0078
///one ancestor entry: parent node's position + the child slot we descended through.
#[derive(Clone, Copy, Debug)]
pub struct Ancestor {
    pub parent: Pos,
    pub child:  ChildPos,
}
///L0087
///stackful walker's ancestor stack, one entry per level. stores pos (not addr): fixup
///applies `fix_pos` directly, no translator; O(height) per op.
///todo : optimization : ancestry is sorted for preorder and postorder, those shouldnt have to check every item every time.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    pub stack: Vec<Ancestor>,
}
///L0095
///pos + ancestry — the standard stackful walker state: satisfies the
///`NodeCursor::State` (`CursorState`) contract for any stackful walker, so
///consumers embed it instead of reimplementing the fixup loop.
#[derive(Clone, Debug, Default)]
pub struct PosAncestry {
    pub pos:      Pos,
    pub ancestry: Ancestry,
}
///L0106
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
///L0120
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
}
///L0132
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
///L0141
///block data that exposes a movable root position + the tree height (splits' root
///promotion bumps it; the consumer's `is_leaf` reads it). extends `Fixable`.
pub trait HasRoot<A: Addr>: Fixable<A> {
    fn root(&self) -> Pos;
    fn set_root(&mut self, root: Pos);
    fn height(&self) -> u32;
    fn set_height(&mut self, height: u32);
}
///L0148
impl Pos {}
///L0160
impl Add<usize> for Pos {}
///L0168
impl Sub<usize> for Pos {}
///L0176
impl Add<usize> for ChildPos {}
///L0184
impl Sub<usize> for ChildPos {}
///L0192
impl PartialEq<usize> for ChildPos {}
///L0199
impl PartialOrd<usize> for ChildPos {}
///L0207
///whole-store remap — every position moves.
impl Fixup for GrewFixup {}
///L0224
///only the run between `from` and `to` shifts; the gap slot at `from` is
///vacated, not moved. `from == to` ⇒ nothing moves.
impl Fixup for NoneSlide {}
///L0245
impl SwapFixup {}
///L0254
///only the moved record remaps.
impl Fixup for SwapFixup {}
///L0270
///affected by either run.
impl Fixup for DoubleSlide {}
///L0294
impl Ancestry {}
///L0312
impl<A: Addr> Fixable<A> for Pos {}
///L0334
///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Height {}
///L0342
///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Depth {}
///L0349
impl<A: Addr> Fixable<A> for Root {}
///L0370
impl<A: Addr> Fixable<A> for Ancestry {}
///L0399
impl<A: Addr> Fixable<A> for PosAncestry {}
///L0425
///blanket: pointer-free block data.
impl<A: Addr> Fixable<A> for () {}
///L0432
impl<A: Addr> CursorState<A> for Pos {}
///L0442
impl<A: Addr> CursorState<A> for PosAncestry {}
///L0454
impl<A: Addr> HasRoot<A> for Root {}
```
