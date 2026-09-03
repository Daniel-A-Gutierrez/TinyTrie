use crate::{index::BlockIndex, store::NoneSlide, translator::{AddressTranslator, Translator}};

///address-rewriting fixup handed back by a block op (grow/spread ⇒ `GrewFixup`, slide ⇒
///`NoneSlide`). `Fixable` data receives one and corrects the pointers it holds.
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

///spread remap `p → p<<shl + shift_offset` (grow doubles the store; vaddrs stay stable).
pub struct GrewFixup {
    pub shl: u32,
    pub shift_offset: u8,
}

impl Fixup for GrewFixup {
    fn fix_p(&self, p: &mut usize) {
        *p <<= self.shl;
        *p += self.shift_offset as usize;
    }
    //spread remaps the whole store
    fn affects_p(&self, _: usize) -> bool { true }
}

impl Fixup for NoneSlide {
    fn fix_p(&self, p: &mut usize) {
        *p = p.wrapping_add(self.delta as usize); //delta=-1 ⇒ usize::MAX ⇒ p-1
    }
    //only the run between `from` and `to` shifts; the gap slot at `from` is vacated, not moved.
    fn affects_p(&self, p: usize) -> bool {
        if self.from == self.to { return false; }
        let (lo, hi) = (self.from.min(self.to), self.from.max(self.to));
        if self.delta > 0 { lo <= p && p < hi }   //None moves left ⇒ items shift right
        else { lo < p && p <= hi }                //None moves right ⇒ items shift left
    }
}

///tracked tree state (block data, walker data) holding addresses. updates itself from any
///`Fixup` implementor, skipping pointers the fixup reports as unaffected.
pub trait Fixable<P: BlockIndex> {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, tr: &Translator<P>);
}

///block data that exposes a movable root vaddr. extends `Fixable` (root must be fixed up).
pub trait HasRoot<P: BlockIndex>: Fixable<P> {
    fn root(&self) -> usize;
    fn set_root(&mut self, root: usize);
}

// ---- default metadata types ----

///tree height for fixed-height trees (b+ / S trees). pointer-free. fixup is noop
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Height(pub u64);
impl<P: BlockIndex> Fixable<P> for Height {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}

///walker's current depth. pointer-free. fixup is noop.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth(pub u64);
impl<P: BlockIndex> Fixable<P> for Depth {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}

///minimal block data: a single root vaddr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root(usize);
impl Default for Root {
    fn default() -> Self { Self(0) }
}
impl<P: BlockIndex> Fixable<P> for Root {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, _: &Translator<P>) {
        if f.affects_p(self.0) { f.fix_p(&mut self.0); }
    }
}
impl<P: BlockIndex> HasRoot<P> for Root {
    fn root(&self) -> usize { self.0 }
    fn set_root(&mut self, root: usize) { self.0 = root; }
}

///one ancestor entry: parent node's phys slot + the child index we descended through.
#[derive(Clone, Copy, Debug)]
pub struct Ancestor {
    pub parent: usize,
    pub child: usize,
}

///stackful walker's ancestor stack, one entry per level. stores phys (not vaddr): fixup
///applies `fix_p` directly, no translator; O(height) per op.
///todo : optimization : ancestry is sorted for preorder and postorder, those shouldnt have to check every item every time.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    pub stack: Vec<Ancestor>,
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
            if f.affects_p(a.parent) { f.fix_p(&mut a.parent); }
        }
    }
}

///pos + ancestry — the standard stackful walker state: satisfies the
///`NodeWalkerMut::State` (`Fixable` + `Clone`) contract for any stackful walker, so
///consumers embed it instead of reimplementing the fixup loop.
#[derive(Clone, Debug, Default)]
pub struct PosAncestry {
    pub pos:      usize,
    pub ancestry: Ancestry,
}
impl<P: BlockIndex> Fixable<P> for PosAncestry {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, tr: &Translator<P>) {
        if f.affects_p(self.pos) {
            f.fix_p(&mut self.pos);
        }
        self.ancestry.fixup(f, tr);
    }
}

///blanket: pointer-free block data.
impl<P: BlockIndex> Fixable<P> for () {
    fn fixup<F: Fixup + ?Sized>(&mut self, _: &F, _: &Translator<P>) {}
}