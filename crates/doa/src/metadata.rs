//!the fixup protocol: block ops hand back fixup structs (`GrewFixup`, `SwapFixup`,
//!`NoneSlide`, `DoubleSlide`), each impling `Fixup` — the uniform contract
//!(`affects_pos`/`fix_pos` direct; `affects_addr`/`fix_addr` through the block's
//!translator). tracked state (block data, walker state) impls `Fixable` — one
//!method per fixup kind, called unconditionally by the walker with the block's
//!translator; the impl decides relevance. also the cursor/walker/block data
//!types (`Pos`, `ChildPos`, `PosAncestry`, `Root`, `Ancestry`, `CursorState`, ...).

use crate::{index::Addr,
            store::NoneSlide,
            translator::{AddressTranslator, Translator}};
use std::cmp::Ordering;
use std::ops::{Add, Sub};

///spread remap `pos → pos<<shl + shift_offset` (grow doubles the store; addrs
///stay stable). `{shl: 0, shift_offset: 1}` doubles as the plain `pos → pos+1`
///(Pluripotent front-edge grow).
#[derive(Clone, Copy)]
pub struct GrewFixup {
    pub shl:          u32,
    pub shift_offset: u8,
}

///a swap exchanged the record at `from` with the None at `to`. only the moved
///record's position remaps (from → to). swaps emit no self-fixup — the mover applies
///this by hand to block data + walker state, and `split_root` returns it as the
///old-root→new-root remap for external addr holders (arena parents) to apply.
#[derive(Clone, Copy, Debug)]
pub struct SwapFixup {
    pub from: Pos,
    pub to:   Pos,
}

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

///a slot in the store's array — the truth: pos 0 holds the min element, pos
///len−1 the max; addr→pos is the translator's job. also the stackless cursor
///state: descends freely (no per-level record), `ascend`/`parent` report
///nothing. derived `Ord` + usize-rhs `+`/`-` keep inline arithmetic bare;
///`.0` for the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos(pub usize);

///a child's slot within its parent's child sequence; as an insertion gap it
///may equal `child_count`. compares against usize (`idx + 1 < child_count()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChildPos(pub usize);

///tree height for fixed-height trees (b+ / S trees). pointer-free no-op-fixable
///level counter — a component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);

///walker's current depth. pointer-free no-op-fixable level counter — a
///component, not a walker state.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);

///minimal tree block data: root position + tree height (Fixable + HasRoot). not a
///walker state — a `reposition` on block data would rewrite the block root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    root:   Pos,
    height: u32,
}

///one ancestor entry: parent node's position + the child slot we descended through.
#[derive(Clone, Copy, Debug)]
pub struct Ancestor {
    pub parent: Pos,
    pub child:  ChildPos,
}

///stackful walker's ancestor stack, one entry per level. stores pos (not addr): fixup
///applies `fix_pos` directly, no translator; O(height) per op.
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
    pub pos:      Pos,
    pub ancestry: Ancestry,
}

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

///block data that exposes a movable root position + the tree height (splits' root
///promotion bumps it; the consumer's `is_leaf` reads it). extends `Fixable`.
pub trait HasRoot<A: Addr>: Fixable<A> {
    fn root(&self) -> Pos;
    fn set_root(&mut self, root: Pos);
    fn height(&self) -> u32;
    fn set_height(&mut self, height: u32);
}

impl Pos {
    #[inline]
    pub fn wrapping_add(self, rhs: usize) -> Self {
        Self(self.0.wrapping_add(rhs))
    }

    #[inline]
    pub fn abs_diff(self, other: Self) -> usize {
        self.0.abs_diff(other.0)
    }
}

impl Add<usize> for Pos {
    type Output = Self;
    #[inline]
    fn add(self, rhs: usize) -> Self {
        Self(self.0 + rhs)
    }
}

impl Sub<usize> for Pos {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: usize) -> Self {
        Self(self.0 - rhs)
    }
}

impl Add<usize> for ChildPos {
    type Output = Self;
    #[inline]
    fn add(self, rhs: usize) -> Self {
        Self(self.0 + rhs)
    }
}

impl Sub<usize> for ChildPos {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: usize) -> Self {
        Self(self.0 - rhs)
    }
}

impl PartialEq<usize> for ChildPos {
    #[inline]
    fn eq(&self, other: &usize) -> bool {
        self.0 == *other
    }
}

impl PartialOrd<usize> for ChildPos {
    #[inline]
    fn partial_cmp(&self, other: &usize) -> Option<Ordering> {
        Some(self.0.cmp(other))
    }
}

///whole-store remap — every position moves.
impl Fixup for GrewFixup {
    fn affects_pos(&self, _: Pos) -> bool {
        true
    }
    fn fix_pos(&self, pos: &mut Pos) {
        pos.0 <<= self.shl;
        pos.0 += self.shift_offset as usize;
    }
    //the translator's knobs absorbed the move — addrs stay stable by construction.
    fn affects_addr<A: Addr>(&self, _: A, _: &Translator<A>) -> bool {
        false
    }
    fn fix_addr<A: Addr>(&self, _: &mut A, _: &Translator<A>) {}
}

///only the run between `from` and `to` shifts; the gap slot at `from` is
///vacated, not moved. `from == to` ⇒ nothing moves.
impl Fixup for NoneSlide {
    fn affects_pos(&self, pos: Pos) -> bool {
        if self.from == self.to {
            return false;
        }
        let (lo, hi) = (self.from.min(self.to), self.from.max(self.to));
        //None moves left ⇒ items shift right (delta > 0); mirror for delta < 0.
        if self.delta > 0 { lo <= pos && pos < hi } else { lo < pos && pos <= hi }
    }
    fn fix_pos(&self, pos: &mut Pos) {
        *pos = pos.wrapping_add(self.delta as usize); //delta=-1 ⇒ usize::MAX ⇒ pos-1
    }
    //slides turn no knobs: the new name is the new slot's, same translator.
    fn affects_addr<A: Addr>(&self, addr: A, tr: &Translator<A>) -> bool {
        self.affects_pos(tr.a2p(addr))
    }
    fn fix_addr<A: Addr>(&self, addr: &mut A, tr: &Translator<A>) {
        *addr = tr.p2a(tr.a2p(*addr).wrapping_add(self.delta as usize));
    }
}

impl SwapFixup {
    ///`from == to` — a remap that moves nothing (the new root kept the old
    ///root's position; external holders apply it as a no-op).
    pub fn no_op(pos: Pos) -> Self {
        Self { from: pos, to: pos }
    }
}

///only the moved record remaps.
impl Fixup for SwapFixup {
    fn affects_pos(&self, pos: Pos) -> bool {
        pos == self.from
    }
    fn fix_pos(&self, pos: &mut Pos) {
        *pos = self.to;
    }
    fn affects_addr<A: Addr>(&self, addr: A, tr: &Translator<A>) -> bool {
        tr.a2p(addr) == self.from
    }
    fn fix_addr<A: Addr>(&self, addr: &mut A, tr: &Translator<A>) {
        *addr = tr.p2a(self.to)
    }
}

///affected by either run.
impl Fixup for DoubleSlide {
    fn affects_pos(&self, pos: Pos) -> bool {
        self.a.affects_pos(pos) || self.b.affects_pos(pos)
    }
    //routes to the affected run — disjoint, at most one applies.
    fn fix_pos(&self, pos: &mut Pos) {
        if self.a.affects_pos(*pos) {
            self.a.fix_pos(pos);
        } else if self.b.affects_pos(*pos) {
            self.b.fix_pos(pos);
        }
    }
    fn affects_addr<A: Addr>(&self, addr: A, tr: &Translator<A>) -> bool {
        self.a.affects_addr(addr, tr) || self.b.affects_addr(addr, tr)
    }
    fn fix_addr<A: Addr>(&self, addr: &mut A, tr: &Translator<A>) {
        if self.a.affects_addr(*addr, tr) {
            self.a.fix_addr(addr, tr);
        } else if self.b.affects_addr(*addr, tr) {
            self.b.fix_addr(addr, tr);
        }
    }
}

impl Ancestry {
    pub fn push(&mut self, parent: Pos, child: ChildPos) {
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

impl<A: Addr> Fixable<A> for Pos {
    fn grew_fix(&mut self, fix: GrewFixup, _: &Translator<A>) {
        fix.fix_pos(self);
    }
    fn swap_fix(&mut self, fix: SwapFixup, _: &Translator<A>) {
        if fix.affects_pos(*self) {
            fix.fix_pos(self);
        }
    }
    fn slide_fix(&mut self, fix: NoneSlide, _: &Translator<A>) {
        if fix.affects_pos(*self) {
            fix.fix_pos(self);
        }
    }
    fn two_slide(&mut self, fix: DoubleSlide, _: &Translator<A>) {
        if fix.affects_pos(*self) {
            fix.fix_pos(self);
        }
    }
}

///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Height {
    fn grew_fix(&mut self, _: GrewFixup, _: &Translator<A>) {}
    fn swap_fix(&mut self, _: SwapFixup, _: &Translator<A>) {}
    fn slide_fix(&mut self, _: NoneSlide, _: &Translator<A>) {}
    fn two_slide(&mut self, _: DoubleSlide, _: &Translator<A>) {}
}

///pointer-free: nothing to fix.
impl<A: Addr> Fixable<A> for Depth {
    fn grew_fix(&mut self, _: GrewFixup, _: &Translator<A>) {}
    fn swap_fix(&mut self, _: SwapFixup, _: &Translator<A>) {}
    fn slide_fix(&mut self, _: NoneSlide, _: &Translator<A>) {}
    fn two_slide(&mut self, _: DoubleSlide, _: &Translator<A>) {}
}

impl<A: Addr> Fixable<A> for Root {
    fn grew_fix(&mut self, fix: GrewFixup, _: &Translator<A>) {
        fix.fix_pos(&mut self.root);
    }
    fn swap_fix(&mut self, fix: SwapFixup, _: &Translator<A>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
    fn slide_fix(&mut self, fix: NoneSlide, _: &Translator<A>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
    fn two_slide(&mut self, fix: DoubleSlide, _: &Translator<A>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
}

impl<A: Addr> Fixable<A> for Ancestry {
    fn grew_fix(&mut self, fix: GrewFixup, _: &Translator<A>) {
        for a in &mut self.stack {
            fix.fix_pos(&mut a.parent);
        }
    }
    fn swap_fix(&mut self, fix: SwapFixup, _: &Translator<A>) {
        for a in &mut self.stack {
            if fix.affects_pos(a.parent) {
                fix.fix_pos(&mut a.parent);
            }
        }
    }
    fn slide_fix(&mut self, fix: NoneSlide, _: &Translator<A>) {
        for a in &mut self.stack {
            if fix.affects_pos(a.parent) {
                fix.fix_pos(&mut a.parent);
            }
        }
    }
    fn two_slide(&mut self, fix: DoubleSlide, _: &Translator<A>) {
        for a in &mut self.stack {
            if fix.affects_pos(a.parent) {
                fix.fix_pos(&mut a.parent);
            }
        }
    }
}

impl<A: Addr> Fixable<A> for PosAncestry {
    fn grew_fix(&mut self, fix: GrewFixup, tr: &Translator<A>) {
        fix.fix_pos(&mut self.pos);
        self.ancestry.grew_fix(fix, tr);
    }
    fn swap_fix(&mut self, fix: SwapFixup, tr: &Translator<A>) {
        if fix.affects_pos(self.pos) {
            fix.fix_pos(&mut self.pos);
        }
        self.ancestry.swap_fix(fix, tr);
    }
    fn slide_fix(&mut self, fix: NoneSlide, tr: &Translator<A>) {
        if fix.affects_pos(self.pos) {
            fix.fix_pos(&mut self.pos);
        }
        self.ancestry.slide_fix(fix, tr);
    }
    fn two_slide(&mut self, fix: DoubleSlide, tr: &Translator<A>) {
        if fix.affects_pos(self.pos) {
            fix.fix_pos(&mut self.pos);
        }
        self.ancestry.two_slide(fix, tr);
    }
}

///blanket: pointer-free block data.
impl<A: Addr> Fixable<A> for () {
    fn grew_fix(&mut self, _: GrewFixup, _: &Translator<A>) {}
    fn swap_fix(&mut self, _: SwapFixup, _: &Translator<A>) {}
    fn slide_fix(&mut self, _: NoneSlide, _: &Translator<A>) {}
    fn two_slide(&mut self, _: DoubleSlide, _: &Translator<A>) {}
}

impl<A: Addr> CursorState<A> for Pos {
    fn position(&self) -> Pos {
        *self
    }
    fn reposition(&mut self, pos: Pos) {
        *self = pos;
    }
    fn descend(&mut self, _: Pos, _: ChildPos) {}
}

impl<A: Addr> CursorState<A> for PosAncestry {
    fn position(&self) -> Pos {
        self.pos
    }
    fn reposition(&mut self, pos: Pos) {
        self.pos = pos;
    }
    fn descend(&mut self, parent: Pos, child: ChildPos) {
        self.ancestry.push(parent, child);
    }
}

impl<A: Addr> HasRoot<A> for Root {
    fn root(&self) -> Pos {
        self.root
    }
    fn set_root(&mut self, root: Pos) {
        self.root = root;
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn set_height(&mut self, height: u32) {
        self.height = height;
    }
}
