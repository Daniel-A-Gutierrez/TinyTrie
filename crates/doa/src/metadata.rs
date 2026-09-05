//!the fixup protocol: block ops hand back `Fixup` implementors; any tracked
//!state (`BlockData`, walker state) that impls `Fixable` receives one and
//!corrects the addresses it holds. also the walker/block data types (`Pos`,
//!`PosAncestry`, `Root`, `Ancestry`, ...).

use crate::{index::BlockIndex,
            store::NoneSlide,
            translator::{AddressTranslator, Translator}};

///spread remap `p → p<<shl + shift_offset` (grow doubles the store; vaddrs
///stay stable). `{shl: 0, shift_offset: 1}` doubles as the plain `p → p+1`
//(Pluripotent front-edge grow).
pub struct GrewFixup {
    pub shl:          u32,
    pub shift_offset: u8,
}

///a swap exchanged the record at `from` with the None at `to`. only the moved
///record's phys remaps (from → to). swaps emit no self-fixup — the mover applies
///this by hand to block data + walker state, and `split_root` returns it as the
///old-root→new-root remap for external vaddr holders (arena parents) to apply.
#[derive(Clone, Copy, Debug)]
pub struct SwapFixup {
    pub from: usize,
    pub to:   usize,
}

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

///bare walker position — the stackless cursor state: descends freely (no
///per-level record), `ascend`/`parent` report nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos(pub usize);

///tree height for fixed-height trees (b+ / S trees). pointer-free no-op-fixable
///level counter — a component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);

///walker's current depth. pointer-free no-op-fixable level counter — a
///component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);

///minimal tree block data: root phys + tree height (Fixable + HasRoot). not a
///walker state — a `set_position` on block data would rewrite the block root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    root:   usize,
    height: u32,
}

///one ancestor entry: parent node's phys slot + the child index we descended through.
#[derive(Clone, Copy, Debug)]
pub struct Ancestor {
    pub parent: usize,
    pub child:  usize,
}

///stackful walker's ancestor stack, one entry per level. stores phys (not vaddr): fixup
///applies `fix_p` directly, no translator; O(height) per op.
///todo : optimization : ancestry is sorted for preorder and postorder, those shouldnt have to check every item every time.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    pub stack: Vec<Ancestor>,
}

///pos + ancestry — the standard stackful walker state: satisfies the
///`NodeCursor::State` (`CursorState`) contract for any stackful walker, so
///consumers embed it instead of reimplementing the fixup loop.
#[derive(Clone, Debug, Default)]
pub struct PosAncestry {
    pub pos:      usize,
    pub ancestry: Ancestry,
}

///address-rewriting fixup handed back by a block op (grow/spread ⇒ `GrewFixup`,
///slide ⇒ `NoneSlide`, swap ⇒ `SwapFixup`). `Fixable` data receives one and
///corrects the pointers it holds.
pub trait Fixup {
    ///rewrite a physical slot index.
    fn fix_p(&self, p: &mut usize);
    ///rewrite a vaddr via the translator. default: translate, `fix_p`, translate back.
    fn fix_v<P: BlockIndex>(&self, v: &mut P, a: &Translator<P>) {
        let mut p = a.v2p(*v);
        self.fix_p(&mut p);
        *v = a.p2v(p);
    }
    ///does this fixup move the record at phys `p`? lets `Fixable` skip untouched pointers.
    fn affects_p(&self, p: usize) -> bool;
    ///vaddr variant — default: translate and ask `affects_p`.
    fn affects_v<P: BlockIndex>(&self, v: P, a: &Translator<P>) -> bool {
        self.affects_p(a.v2p(v))
    }
}

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
    fn descend(&mut self, parent: usize, child_idx: usize) {
        let _ = (parent, child_idx);
    }
}

///tracked tree state (block data, walker data) holding addresses. updates itself from any
///`Fixup` implementor, skipping pointers the fixup reports as unaffected.
pub trait Fixable<P: BlockIndex> {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, tr: &Translator<P>);
}

///block data that exposes a movable root phys + the tree height (splits' root
///promotion bumps it; the consumer's `is_leaf` reads it). extends `Fixable`.
pub trait HasRoot<P: BlockIndex>: Fixable<P> {
    fn root(&self) -> usize;
    fn set_root(&mut self, root: usize);
    fn height(&self) -> u32;
    fn set_height(&mut self, height: u32);
}

impl Fixup for GrewFixup {
    fn fix_p(&self, p: &mut usize) {
        *p <<= self.shl;
        *p += self.shift_offset as usize;
    }
    //spread remaps the whole store
    fn affects_p(&self, _: usize) -> bool {
        true
    }
}

impl Fixup for NoneSlide {
    fn fix_p(&self, p: &mut usize) {
        *p = p.wrapping_add(self.delta as usize); //delta=-1 ⇒ usize::MAX ⇒ p-1
    }
    //only the run between `from` and `to` shifts; the gap slot at `from` is vacated, not moved.
    fn affects_p(&self, p: usize) -> bool {
        if self.from == self.to {
            return false;
        }
        let (lo, hi) = (self.from.min(self.to), self.from.max(self.to));
        if self.delta > 0 { lo <= p && p < hi }   //None moves left ⇒ items shift right
        else { lo < p && p <= hi } //None moves right ⇒ items shift left
    }
}

impl SwapFixup {
    ///`from == to` — a remap that moves nothing (the new root kept the old
    ///root's address; external holders apply it as a no-op).
    pub fn no_op(p: usize) -> Self {
        Self { from: p, to: p }
    }
}

impl Fixup for SwapFixup {
    fn fix_p(&self, p: &mut usize) {
        if *p == self.from {
            *p = self.to;
        }
    }
    fn affects_p(&self, p: usize) -> bool {
        p == self.from
    }
}

impl Fixup for TwoSlide {
    fn fix_p(&self, p: &mut usize) {
        self.a.fix_p(p);
        self.b.fix_p(p);
    }
    fn affects_p(&self, p: usize) -> bool {
        self.a.affects_p(p) || self.b.affects_p(p)
    }
}

impl<P: BlockIndex> Fixable<P> for Pos {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, _: &Translator<P>) {
        if f.affects_p(self.0) {
            f.fix_p(&mut self.0);
        }
    }
}

impl<P: BlockIndex> CursorState<P> for Pos {
    fn position(&self) -> usize {
        self.0
    }
    fn reposition(&mut self, pos: usize) {
        self.0 = pos;
    }
}

impl<P: BlockIndex> Fixable<P> for Height {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}

impl<P: BlockIndex> Fixable<P> for Depth {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}

impl Default for Root {
    fn default() -> Self {
        Self { root: 0, height: 0 }
    }
}

impl<P: BlockIndex> Fixable<P> for Root {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, _: &Translator<P>) {
        if f.affects_p(self.root) {
            f.fix_p(&mut self.root);
        }
    }
}

impl<P: BlockIndex> HasRoot<P> for Root {
    fn root(&self) -> usize {
        self.root
    }
    fn set_root(&mut self, root: usize) {
        self.root = root;
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn set_height(&mut self, height: u32) {
        self.height = height;
    }
}

impl Ancestry {
    pub fn push(&mut self, parent: usize, child: usize) {
        self.stack.push(Ancestor { parent, child });
    }
    pub fn pop(&mut self) -> Option<Ancestor> {
        self.stack.pop()
    }
    pub fn last(&self) -> Option<&Ancestor> {
        self.stack.last()
    }
    pub fn len(&self) -> usize {
        self.stack.len()
    }
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

impl<P: BlockIndex> Fixable<P> for Ancestry {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, _: &Translator<P>) {
        for a in &mut self.stack {
            if f.affects_p(a.parent) {
                f.fix_p(&mut a.parent);
            }
        }
    }
}

impl<P: BlockIndex> Fixable<P> for PosAncestry {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, tr: &Translator<P>) {
        if f.affects_p(self.pos) {
            f.fix_p(&mut self.pos);
        }
        self.ancestry.fixup(f, tr);
    }
}

impl<P: BlockIndex> CursorState<P> for PosAncestry {
    fn position(&self) -> usize {
        self.pos
    }
    fn reposition(&mut self, pos: usize) {
        self.pos = pos;
    }
    fn descend(&mut self, parent: usize, child_idx: usize) {
        self.ancestry.push(parent, child_idx);
    }
}

///blanket: pointer-free block data.
impl<P: BlockIndex> Fixable<P> for () {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}
