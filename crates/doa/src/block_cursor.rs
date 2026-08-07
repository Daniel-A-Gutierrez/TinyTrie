use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::block::*;
use crate::store::{NoneSlide, Store};
use crate::translator::*;
use crate::{Fixup, index::*};

///positioned reader over a block's `Some` slots. the cursor tracks a **physical**
///slot internally. `T: 'cursor` is the arena validity bound. returned `&T` tie to the
///`&self`/`&mut self` call borrow (not `'cursor`): navigate, read, advance — not a
///streaming iterator.
pub trait Cursor<'cursor, T: 'cursor, P: BlockIndex> {
    ///vaddr of the current element, or `None` if at-end.
    fn address(&self) -> Option<P>;
    ///physical index in the store
    fn position(&self) -> Option<usize>;
    ///current element, or `None` if at-end.
    fn current(&self) -> Option<&T>;
    ///O(1) jump to vaddr `v` (translated to phys); panics if out of bounds or `None`.
    ///returns the element now under the cursor.
    fn seek(&mut self, v: P) -> Option<&T>;
    ///advance to the next `Some` slot. returns the new element, or `None` if at-end.
    fn next(&mut self) -> Option<&T>;
    ///advance to the previous `Some` slot. returns the new element, or `None` if at the first.
    fn prev(&mut self) -> Option<&T>;
    ///seek to the first occupied slot; returns its element, or `None` if empty.
    fn first(&mut self) -> Option<&T>;
    ///seek to the last occupied slot; returns its element, or `None` if empty.
    fn last(&mut self) -> Option<&T>;
    fn p2v(&self, phys: usize) -> P ;
    fn v2p(&self, v: P) -> usize ;
}

/// positioned mut reader over a block's `Some` slots.
///`'cursor` = the block borrow the cursor holds (`'block: 'cursor`); returned
/// `&mut T` are tied to the `&mut self` call borrow (≤ `'cursor`), so it is not a
/// streaming iterator — call, use, drop, advance.
pub trait CursorMut<'cursor, T: 'cursor, P: BlockIndex> : Cursor<'cursor,T,P> {
    fn current_mut(&mut self) -> Option<&mut T>;
}

///block-backed cursor: holds a block ref (`&B` or `&mut B`) via `R`, scans the
///store's slots through `R: Deref`, translates phys↔vaddr through `block.translator()`.
///`R = &'cursor B` gives a shared read cursor; `R = &'cursor mut B` gives a mut cursor.
pub struct BlockCursor<'block, 'cursor, B: BlockTrait<'block>, R>
where
    'block: 'cursor,
    R: Deref<Target = B> + 'cursor,
{
    block: R,
    pos: Option<usize>,
    _m: PhantomData<(&'block (), &'cursor ())>,
}

impl<'block, 'cursor, B: BlockTrait<'block>, R: Deref<Target = B>>
    BlockCursor<'block, 'cursor, B, R>
where
    'block: 'cursor,
    R: 'cursor,
{
    pub(crate) fn new(block: R) -> Self {
        let mut c = Self { block, pos: None, _m: PhantomData };
        let _ = c.first();
        c
    }

    ///decompose: yield the block ref back plus the current vaddr (stable across
    ///store mutations — the translator remaps, so the same vaddr survives a slide/
    ///spread). `None` if the cursor was at-end.
    pub(crate) fn into_parts(self) -> (R, Option<B::P>) {
        let v = self.pos.map(|phys| self.block.translator().p2v(phys));
        (self.block, v)
    }

    ///rebuild by detranslating the stable vaddr back to a (possibly new) phys.
    ///`v == None` rebuilds an at-end cursor.
    pub(crate) fn from_parts(block: R, v: Option<B::P>) -> Self {
        let pos = v.map(|v| block.translator().v2p(v));
        Self { block, pos, _m: PhantomData }
    }

    ///position at vaddr `v`. if its slot is empty (e.g. an unpopulated root) the
    ///cursor is at-end (`pos = None`) rather than seeking a `None` slot.
    pub(crate) fn new_at(block: R, v: B::P) -> Self {
        let phys = block.translator().v2p(v);
        let pos = (phys < block.store().len() && block.store().slot(phys).is_some()).then_some(phys);
        Self { block, pos, _m: PhantomData }
    }

    pub(crate) fn p2v(&self, phys: usize) -> B::P {
        self.block.p2v(phys)
    }
    pub(crate) fn v2p(&self, v: B::P) -> usize {
        self.block.v2p(v)
    }
}

impl<'block, 'cursor, B: BlockMutTrait<'block>, R: DerefMut<Target = B>>
    BlockCursor<'block, 'cursor, B, R>
where
    'block: 'cursor,
    R: 'cursor,
{
    ///find a free slot or grow. `pos`/`pin` are physical and NOT the cursor's tracked
    ///element. applies the block's grew fixup to the tracked element so it survives the
    ///spread. the pending slide (`found.slide`) is NOT applied here — call `slide_none`
    ///with it to perform the shift.
    pub(crate) fn find_slot(&mut self, pos: usize, dir: bool, pin: Option<usize>) -> FoundSlot {
        let found = self.block.find_slot(pos, dir, pin);
        if let Some(grew) = &found.grew
            && let Some(phys) = self.pos.as_mut()
        {
            grew.fix_p(phys);
        }
        found
    }

    ///perform the slide. `pin` is physical. the tracked element shifts by the slide's
    ///delta iff it lies in the moved run (between `ns.from` and `ns.to`, exclusive of the
    ///None at `ns.from`); the pin is kept out of the run by `find_slot`, so a tracked pin
    ///stays put.
    pub(crate) fn slide_none(&mut self, ns: NoneSlide, pin: Option<usize>) -> OpenSlot {
        let opened = self.block.slide_none(ns, pin);
        if let Some(phys) = self.pos.as_mut() {
            let lo = ns.from.min(ns.to);
            let hi = ns.from.max(ns.to);
            if *phys != ns.from && *phys >= lo && *phys <= hi {
                ns.fix_p(phys);
            }
        }
        opened
    }

    ///place `t` at the opened slot; returns its phys. insert fills a None slot and moves
    ///no other element (auto-grow lives in `find_slot`), so the tracked phys is unchanged.
    pub(crate) fn insert(&mut self, t: B::T, slot: OpenSlot) -> usize {
        self.block.insert(t, slot)
    }

    ///remove the element at phys `phys`. if it was the tracked element, the cursor goes at-end.
    pub(crate) fn remove(&mut self, phys: usize) -> B::T {
        let t = self.block.remove(phys);
        if self.pos == Some(phys) {
            self.pos = None;
        }
        t
    }

    ///swap the contents at two phys slots. the tracked element follows its data: if at
    ///`a` it moves to `b`, and vice versa.
    pub(crate) fn swap(&mut self, a: usize, b: usize) {
        self.block.swap(a, b);
        match self.pos {
            Some(phys) if phys == a => self.pos = Some(b),
            Some(phys) if phys == b => self.pos = Some(a),
            _ => {}
        }
    }

    ///swap the record at phys `src` with the None at `open`. returns the freed slot at
    ///`src`'s phys and the phys the record moved to. the tracked element follows: if at
    ///`src` it is now at the returned phys.
    pub(crate) fn swap_open(&mut self, src: usize, open: OpenSlot) -> (OpenSlot, usize) {
        let (freed, new_phys) = self.block.swap_open(src, open);
        if self.pos == Some(src) {
            self.pos = Some(new_phys);
        }
        (freed, new_phys)
    }
}

impl<'block, 'cursor, B: BlockTrait<'block>, R: Deref<Target = B>> Cursor<'cursor, B::T, B::P>
    for BlockCursor<'block, 'cursor, B, R>
where
    'block: 'cursor,
    R: 'cursor,
{
    
    fn address(&self) -> Option<B::P> {
        self.pos.map(|phys| self.block.translator().p2v(phys))
    }

    fn position(&self) -> Option<usize> {
        self.pos
    }

    fn current(&self) -> Option<&B::T> {
        let phys = self.pos?;
        Some(self.block.get(phys))
    }

    fn seek(&mut self, v: B::P) -> Option<&B::T> {
        let phys = self.block.translator().v2p(v);
        let n = self.block.store().len();
        assert!(phys < n, "cursor: seek out of bounds");
        assert!(self.block.store().slot(phys).is_some(), "cursor: seek to None");
        self.pos = Some(phys);
        Some(self.block.get(phys))
    }

    fn next(&mut self) -> Option<&B::T> {
        let Some(phys) = self.pos else { return None };
        let s = self.block.store();
        for q in (phys + 1)..s.len() {
            if s.slot(q).is_some() {
                self.pos = Some(q);
                return Some(self.block.get(q));
            }
        }
        self.pos = None;
        None
    }

    fn prev(&mut self) -> Option<&B::T> {
        let Some(phys) = self.pos else { return None };
        let s = self.block.store();
        for q in (0..phys).rev() {
            if s.slot(q).is_some() {
                self.pos = Some(q);
                return Some(self.block.get(q));
            }
        }
        None
    }

    fn first(&mut self) -> Option<&B::T> {
        let s = self.block.store();
        for phys in 0..s.len() {
            if s.slot(phys).is_some() {
                self.pos = Some(phys);
                return Some(self.block.get(phys));
            }
        }
        self.pos = None;
        None
    }

    fn last(&mut self) -> Option<&B::T> {
        let s = self.block.store();
        for phys in (0..s.len()).rev() {
            if s.slot(phys).is_some() {
                self.pos = Some(phys);
                return Some(self.block.get(phys));
            }
        }
        self.pos = None;
        None
    }
    
    fn p2v(&self, phys: usize) -> B::P  {
        self.block.p2v(phys)
    }
    
    fn v2p(&self, v: B::P) -> usize  {
        self.block.v2p(v)
    }
}

impl<'block, 'cursor, B: BlockMutTrait<'block>, R: DerefMut<Target = B>>
    CursorMut<'cursor, B::T, B::P> for BlockCursor<'block, 'cursor, B, R>
where
    'block: 'cursor,
    R: 'cursor,
{
    fn current_mut(&mut self) -> Option<&mut B::T> {
        let phys = self.pos?;
        Some(self.block.get_mut(phys))
    }
}