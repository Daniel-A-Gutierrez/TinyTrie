```rust
//!the fixup protocol: block ops hand back `Fixup` implementors; any tracked
//!state (`BlockData`, walker state) that impls `Fixable` receives one and
//!corrects the addresses it holds. also the walker/block data types (`Pos`,
//!`PosAncestry`, `Root`, `Ancestry`, ...).
//(Pluripotent front-edge grow).
///L0013
///spread remap `p → p<<shl + shift_offset` (grow doubles the store; vaddrs
///stay stable). `{shl: 0, shift_offset: 1}` doubles as the plain `p → p+1`
pub struct GrewFixup {
    pub shl:          u32,
    pub shift_offset: u8,
}
///L0023
///a swap exchanged the record at `from` with the None at `to`. only the moved
///record's phys remaps (from → to). swaps emit no self-fixup — the mover applies
///this by hand to block data + walker state, and `split_root` returns it as the
///old-root→new-root remap for external vaddr holders (arena parents) to apply.
#[derive(Clone, Copy, Debug)]
pub struct SwapFixup {
    pub from: usize,
    pub to:   usize,
}
///L0034
///two non-overlapping slides from one `find_2_slots` — the address fixup for a
///two-slot reservation, so holders get ONE `fixup` call covering both (order-
///independent: disjoint runs). the applying side still slides them separately
///(the run-parent fixups interleave with the slides and cannot compose;
///subtle_bugs.md §9).
#[derive(Clone, Copy, Debug)]
pub struct TwoSlide {
    pub a: NoneSlide,
    pub b: NoneSlide,
}
///L0042
///bare walker position — the stackless cursor state: descends freely (no
///per-level record), `ascend`/`parent` report nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos(pub usize);
///L0047
///tree height for fixed-height trees (b+ / S trees). pointer-free no-op-fixable
///level counter — a component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);
///L0052
///walker's current depth. pointer-free no-op-fixable level counter — a
///component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);
///L0057
///minimal tree block data: root phys + tree height (Fixable + HasRoot). not a
///walker state — a `set_position` on block data would rewrite the block root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    root:   usize,
    height: u32,
}
///L0064
///one ancestor entry: parent node's phys slot + the child index we descended through.
#[derive(Clone, Copy, Debug)]
pub struct Ancestor {
    pub parent: usize,
    pub child:  usize,
}
///L0073
///stackful walker's ancestor stack, one entry per level. stores phys (not vaddr): fixup
///applies `fix_p` directly, no translator; O(height) per op.
///todo : optimization : ancestry is sorted for preorder and postorder, those shouldnt have to check every item every time.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    pub stack: Vec<Ancestor>,
}
///L0081
///pos + ancestry — the standard stackful walker state: satisfies the
///`NodeCursor::State` (`CursorState`) contract for any stackful walker, so
///consumers embed it instead of reimplementing the fixup loop.
#[derive(Clone, Debug, Default)]
pub struct PosAncestry {
    pub pos:      usize,
    pub ancestry: Ancestry,
}
///L0089
///address-rewriting fixup handed back by a block op (grow/spread ⇒ `GrewFixup`,
///slide ⇒ `NoneSlide`, swap ⇒ `SwapFixup`). `Fixable` data receives one and
///corrects the pointers it holds.
pub trait Fixup {
    ///rewrite a physical slot index.
    fn fix_p(&self, p: &mut usize);
    ///rewrite a vaddr via the translator. default: translate, `fix_p`, translate back.
    fn fix_v<P: BlockIndex>(&self, v: &mut P, a: &Translator<P>);
    ///does this fixup move the record at phys `p`? lets `Fixable` skip untouched pointers.
    fn affects_p(&self, p: usize) -> bool;
    ///vaddr variant — default: translate and ask `affects_p`.
    fn affects_v<P: BlockIndex>(&self, v: P, a: &Translator<P>) -> bool;
}
///L0113
///cursor state — the seam for the crate's default cursor methods: position
///tracking + the descent record (descent is the cursor's only way to move —
///the hook belongs here; a stackless state keeps the no-op default). the ascent
///side is deliberately NOT here: where parent knowledge lives is per-shape
///(a stackful state's records vs a parent-pointer node's stored field), so
///`ascend`/`parent` are consumer methods on `NodeWalker`, not state hooks.
///`Fixable` supertrait so the bound propagates to `NodeCursor::State`.
pub trait CursorState<P: BlockIndex>: Fixable<P> + Clone {
    fn position(&self) -> usize;
    fn reposition(&mut self, pos: usize);
    ///record a descent into `child_idx` of `parent`. default: none kept.
    fn descend(&mut self, parent: usize, child_idx: usize);
}
///L0124
///tracked tree state (block data, walker data) holding addresses. updates itself from any
///`Fixup` implementor, skipping pointers the fixup reports as unaffected.
pub trait Fixable<P: BlockIndex> {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, tr: &Translator<P>);
}
///L0130
///block data that exposes a movable root phys + the tree height (splits' root
///promotion bumps it; the consumer's `is_leaf` reads it). extends `Fixable`.
pub trait HasRoot<P: BlockIndex>: Fixable<P> {
    fn root(&self) -> usize;
    fn set_root(&mut self, root: usize);
    fn height(&self) -> u32;
    fn set_height(&mut self, height: u32);
}
///L0137
impl Fixup for GrewFixup {}
///L0148
impl Fixup for NoneSlide {}
///L0163
impl SwapFixup {}
///L0171
impl Fixup for SwapFixup {}
///L0182
impl Fixup for TwoSlide {}
///L0192
impl<P: BlockIndex> Fixable<P> for Pos {}
///L0200
impl<P: BlockIndex> CursorState<P> for Pos {}
///L0209
impl<P: BlockIndex> Fixable<P> for Height {}
///L0213
impl<P: BlockIndex> Fixable<P> for Depth {}
///L0217
impl Default for Root {}
///L0223
impl<P: BlockIndex> Fixable<P> for Root {}
///L0231
impl<P: BlockIndex> HasRoot<P> for Root {}
///L0246
impl Ancestry {}
///L0264
impl<P: BlockIndex> Fixable<P> for Ancestry {}
///L0274
impl<P: BlockIndex> Fixable<P> for PosAncestry {}
///L0283
impl<P: BlockIndex> CursorState<P> for PosAncestry {}
///L0296
///blanket: pointer-free block data.
impl<P: BlockIndex> Fixable<P> for () {}
```
