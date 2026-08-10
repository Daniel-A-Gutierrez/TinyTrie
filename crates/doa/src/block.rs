use crate::{Fixup, alloc_strat::*};
use crate::{InOrder, Ordering, PostOrder, PreOrder,
            index::*,
            store::{DequeStore, NoneSlide, Store, VecStore},
            translator::{AddressTranslator, Translator}};
use std::fmt;
use std::fmt::Write as _;
use std::marker::PhantomData;
use crate::block_cursor::*;
///per-strategy concrete block aliases. `BlockMutTrait` is only impl'd for these four
///(strategy, store) combos, so these are the only `RawBlock` family members that are
///tree-usable as `Inner`. `pub(crate)` because the stores are `pub(crate)`.
pub(crate) type UniformBlock<'a, T, O: Ordering, P, const CAP: usize> =
    RawBlock<'a, T, P, Uniform<O>, VecStore<T, CAP>>;
pub(crate) type PluripotentBlock<'a, T, O: Ordering, P, const CAP: usize> =
    RawBlock<'a, T, P, Pluripotent<O>, DequeStore<T, CAP>>;
pub(crate) type AppendBlock<'a, T, P, const CAP: usize> =
    RawBlock<'a, T, P, Append, VecStore<T, CAP>>;
pub(crate) type PrependBlock<'a, T, P, const CAP: usize> =
    RawBlock<'a, T, P, Prepend, VecStore<T, CAP>>;

///debug rendering aid for a block-stored item. debug-only. the item carries its own
///debug height (`INode::debug_height`); `debug_render` reads it to pick the right
///interpretation of each leaf slot (terminal SlicePtr vs internal child vaddr -> phys).
///carrying the height on the node (rather than walking the tree) keeps debug working
///when the tree is broken mid-fixup — a walk would OOB-panic on an orphaned node.
pub(crate) trait SlotDebug<P: BlockIndex> {
    fn debug_render(&self, tr: &Translator<P>) -> Vec<String>;
}

///apply slide's fixup to the cursor after doing the slide, but to the nodes before doing the slide. 
pub struct FoundSlot {
    pub(crate) grew : Option<GrewFixup>,
    pub(crate) slide : Option<NoneSlide>
}

pub struct GrewFixup {
    shl : u32, //if the block grew, shl=1, otherwise 0. 
    shift_offset : u8 //depends on strategy, items ended up at 2i or 2i+1. this would be 1 in the latter case.
}

pub struct InsufficientMaxCapacity();

impl Fixup for GrewFixup {
    fn fix_p(&self, p: &mut usize) {
        *p <<= self.shl;
        *p += self.shift_offset as usize;
    }
}

///read-only block surface. `T`/`P`/`S` are associated (derived from the impl, i.e.
///from the concrete `RawBlock` family member), so the tree tier can recover them as
///`Inner::T`/`Inner::P`/`Inner::S` without restating them as params.
pub trait BlockTrait<'a>: 'a {
    type T: Sized + 'a;
    type P: BlockIndex;
    type S: Store<'a, Self::T> + 'a;
    type Cursor<'cursor>: Cursor<'cursor, Self::T, Self::P>
    where
        'a: 'cursor,
        Self: 'cursor;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b;

    fn translator<'b>(&'b self) -> &'b Translator<Self::P>;

    ///physical get. panics if the slot is `None` (contract violation — caller
    ///guarantees `p` is occupied).
    fn get<'b>(&'b self, p: usize) -> &'b Self::T
    where 'a: 'b {
        self.store().get(p)
    }

    ///virtual get: translate vaddr→phys. panics if the slot is `None`.
    fn vget<'b>(&'b self, ptr: Self::P) -> &'b Self::T
    where 'a: 'b {
        self.store().get(self.translator().v2p(ptr))
    }

    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'a: 'b {
        let s = self.store();
        let tr = self.translator();
        for p in 0..s.len() {
            if s.slot(p).is_some() {
                return Some(tr.p2v(p));
            }
        }
        None
    }

    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'a: 'b {
        let s = self.store();
        let tr = self.translator();
        for p in (0..s.len()).rev() {
            if s.slot(p).is_some() {
                return Some(tr.p2v(p));
            }
        }
        None
    }

    fn v2p(&self, virt: Self::P) -> usize {
        self.translator().v2p(virt)
    }

    fn p2v(&self, phys: usize) -> Self::P {
        self.translator().p2v(phys)
    }

    fn vdist(&self, v1: Self::P, v2: Self::P) -> usize {
        self.translator().vdist(v1, v2)
    }

    fn occupied<'b>(&'b self) -> usize
    where 'a: 'b {
        self.store().occupied()
    }

    fn len<'b>(&'b self) -> usize
    where 'a: 'b {
        self.store().len()
    }

    fn cap<'b>(&'b self) -> usize
    where 'a: 'b {
        self.store().cap()
    }

    fn max_capacity(&self) -> usize {
        Self::S::max_capacity()
    }

    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Self::T> + 'b
    where 'a: 'b {
        self.store().iter()
    }

    ///block read cursor: positioned at the first occupied slot (or at-end if empty).
    fn cursor<'cursor>(&'cursor self) -> Self::Cursor<'cursor>
    where 'a: 'cursor;
}

///mutation surface. blocks of different alloc strats implement a common interface but failures must be handled at runtime.
///`A` (the strategy) is associated; `T`/`P`/`S` come from the `BlockTrait` supertrait.
pub trait BlockMutTrait<'a>: BlockTrait<'a> {
    type A: AllocStrat<Self::P>;
    type CursorMut<'cursor>: CursorMut<'cursor, Self::T, Self::P>
    where
        'a: 'cursor,
        Self: 'cursor;

    fn new() -> Self;
    fn store_mut(&mut self) -> &mut Self::S;

    fn translator_mut(&mut self) -> &mut Translator<Self::P>;

    ///block mut cursor: positioned at the first occupied slot (or at-end if empty).
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor;

    ///physical mut get. panics if the slot is `None` (contract violation — caller
    ///guarantees `p` is occupied).
    fn get_mut<'b>(&'b mut self, p: usize) -> &'b mut Self::T
    where 'a: 'b {
        self.store_mut().get_mut(p)
    }

    ///virtual mut get: translate vaddr→phys. panics if the slot is `None`.
    fn vget_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::T
    where 'a: 'b {
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }

    ///two disjoint `&mut` to occupied physical slots `a`, `b`. panics if `a == b`
    ///or either is `None`. for `split_into` between two in-block nodes.
    fn get_disjoint_mut<'b>(&'b mut self, a: usize, b: usize) -> (&'b mut Self::T, &'b mut Self::T)
    where 'a: 'b {
        self.store_mut().get_disjoint_mut(a, b)
    }

    ///slide the None `ms.from` -> `ms.to`; returns the opened slot. `pin` (phys) must
    ///not move.
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<usize>) -> OpenSlot {
        OpenSlot(self.store_mut().slide_none(ms, pin))
    }

    ///first insert into an empty block. grows to `INIT_CAP` Nones and lands the root at
    ///the midpoint phys (`INIT_CAP/2`): for in-order (`INIT_CAP=2`) that's phys 1 — the
    ///physical midpoint of the len-2 block. returns the root's phys.
    fn insert_root(&mut self, v: Self::T) -> usize {
        assert!(self.store().len() == 0, "insert_root: block not empty");
        let cap = Self::A::INIT_CAP as usize;
        self.store_mut().grow_back(cap);
        let mid = cap / 2;
        self.store_mut().insert(v, mid);
        mid
    }

    ///manually grow + spread; fails if shift==0 or would exceed max capacity.
    fn grow_and_spread(&mut self) -> Result<GrewFixup,InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > Self::S::max_capacity(){
            return Err(InsufficientMaxCapacity());
        }
        Self::A::on_grow(self.translator_mut());
        self.store_mut().spread(Self::A::SPREAD_OFFSET);
        Ok(GrewFixup { shl: 1, shift_offset: Self::A::SPREAD_OFFSET as u8} )
    }

    ///find free slot or make space if possible. dir is logical (true=after);
    ///REVERSED strategies flip it to phys. `pos`/`pin` are physical.
    fn find_slot(
        &mut self,
        pos: usize,
        dir: bool,
        pin: Option<usize>,
    ) -> FoundSlot {
        let mut found = FoundSlot{grew:None,slide:None};
        let dir = dir ^ Self::A::REVERSED;
        if let Some(ns) = self.store().find_slot(pos, dir, Self::A::INSERT_BUDGET, pin) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == self.max_capacity() {
            return found;
        }
        if let Ok(g) = self.grow_and_spread() {
            found.grew = Some(g);
        }
        //spread remaps phys (i->2i+offset); apply the grew fixup to recover the new phys.
        let mut pos = pos;
        let mut pin = pin;
        if let Some(g) = &found.grew {
            g.fix_p(&mut pos);
            if let Some(p) = pin.as_mut() {
                g.fix_p(p);
            }
        }
        found.slide = self.store().find_slot(pos, dir, self.len(), pin);
        found
    }

    ///place `v` at the opened slot. returns its phys.
    fn insert(&mut self, v: Self::T, slot: OpenSlot) -> usize {
        self.store_mut().insert(v, slot.0);
        slot.0
    }

    fn remove(&mut self, p: usize) -> (Self::T,OpenSlot) {
        (self.store_mut().remove(p),OpenSlot(p))
    }

    ///swap the contents at two phys slots.
    fn swap(&mut self, a: usize, b: usize) {
        self.store_mut().swap(a, b);
    }

    ///swap the record at phys `src` with the None at `open`. returns the slot freed at
    ///`src`'s phys and the phys of the record that was at `src` (now at `open`'s
    ///phys). used to relocate a wired node to a new gap (`hop_to_median`) and to land a new
    ///node at a specific phys (root promote): the caller inserts into the freed slot, or
    ///reads the returned phys to update the node's inbound pointer.
    fn swap_open(&mut self, src: usize, open: OpenSlot) -> (OpenSlot, usize) {
        self.store_mut().swap(src, open.0);
        (OpenSlot(src), open.0)
    }

    //none of the split stuff is really in use or correct or working.

    ///self keeps [0,at).
    ///precondition: len == P::MAX.as_usize() + 1 (block full).
    fn split(&mut self, at : usize) -> Self;

    ///split at 'at' and then spread both sides, add 1 rotation to translator. 
    fn split_and_rotate(&mut self, at : usize) -> Self;

    ///failure is a signal to use a different block or block type.
    ///will not move elements
    fn try_insert_back(&mut self, v: Self::T) -> Result<usize, Self::T>;

    ///failure is a signal to use a different block or block type.
    ///will not move elements
    fn try_insert_front(&mut self, v: Self::T) -> Result<usize, Self::T>;
}

///raw ordered arena run: owns a store + translator, upholds no structural
///invariant.
pub struct RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T>,
{
    _strategy:  PhantomData<A>,
    store:      S,
    translator: Translator<P>,
    _phantom:   PhantomData<&'a T>,
}

pub(crate) struct OpenSlot(pub(crate) usize);

///fwd-or-rev iterator wrapper: REVERSED strategies pick Rev, else Fwd. The dead
///arm is never constructed per-monomorphization (A::REVERSED is const).
pub(crate) enum DirIter<F, R> {
    Fwd(F),
    Rev(R),
}

impl<F: Iterator, R: Iterator<Item = F::Item>> Iterator for DirIter<F, R> {
    type Item = F::Item;
    #[inline]
    fn next(&mut self) -> Option<F::Item> {
        match self {
            Self::Fwd(i) => i.next(),
            Self::Rev(i) => i.next(),
        }
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Fwd(i) => i.size_hint(),
            Self::Rev(i) => i.size_hint(),
        }
    }
}

impl<F: ExactSizeIterator, R: ExactSizeIterator<Item = F::Item>> ExactSizeIterator
    for DirIter<F, R>
{
}

impl<'a, T, P, A, S> BlockTrait<'a> for RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
{
    type T = T;
    type P = P;
    type S = S;
    type Cursor<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor Self>
    where 'a: 'cursor;

    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        &self.store
    }

    fn translator<'b>(&'b self) -> &'b Translator<P> {
        &self.translator
    }

    fn cursor<'cursor>(&'cursor self) -> Self::Cursor<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }

    ///REVERSED strategies iterate high→low (front at the back).
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where 'a: 'b {
        let it = self.store().iter();
        if A::REVERSED { DirIter::Rev(it.rev()) } else { DirIter::Fwd(it) }
    }
}

///`Debug` view: translator params + physical slot layout (`[i:[child_phys,...], j:X, ...]`).
///the full sparse physical layout (gaps included) is rendered by probing each slot via
///`Store::slot`. each Some's child vaddrs (`SlotDebug::debug_render`) are mapped to
///physical slots via `v2p`.
impl<'a, T, P, A, S> fmt::Debug for RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a + SlotDebug<P>,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tr = &self.translator;
        writeln!(
            f,
            "RawBlock {{\n  tr(inner={:?}, outer={:?}, shift={}, rot={})",
            tr.inner_offset(),
            tr.outer_offset(),
            tr.shift(),
            tr.rotation()
        )?;
        let s = self.store();
        let len = s.len();
        let mut buf = String::new();
        buf.push_str("  slots: [");
        for phys in 0..len {
            if phys > 0 {
                buf.push_str(", ");
            }
            match s.slot(phys) {
                Some(item) => {
                    let parts = item.debug_render(tr);
                    let _ = write!(buf, "{phys}:[{}]", parts.join(","));
                }
                None => {
                    let _ = write!(buf, "{phys}:X");
                }
            }
        }
        buf.push(']');
        f.write_str(&buf)?;
        f.write_str("\n}")
    }
}

///strategy-agnostic Self-construction: new + split + split_and_rotate. one copy
///(behavior varies via A for `new`); the per-strategy BlockMutTrait impls delegate
///here so push_* are the only thing that differs by strategy.
impl<'a, T, P, A, S> RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
{
    ///empty block: inner_offset = A::INIT_INNER_OFFSET (anchor w/ headroom on the
    ///non-dominant side); outer_offset = A::INIT_OUTER_OFFSET; shift = A::INIT_SHIFT; rotation 0.
    pub(crate) fn new_block() -> Self {
        let shift = A::INIT_SHIFT;
        debug_assert!(
            S::max_capacity() <= A::CAP_LIMIT,
            "store MAX_CAP exceeds strategy CAP_LIMIT"
        );
        Self {
            _strategy:  PhantomData,
            store:      S::new(),
            translator: Translator::new(
                P::from_usize(A::INIT_INNER_OFFSET),
                P::from_usize(A::INIT_OUTER_OFFSET),
                shift,
                0,
            ),
            _phantom:   PhantomData,
        }
    }

    ///split [at,len) into a new block, cloning the translator. 
    ///precondition: len == P::MAX.as_usize() + 1 (block full).
    ///caller guarantees no nodes present in right point to nodes in left. 
    pub(crate) fn split_block(&mut self, at : usize) -> Self {
        debug_assert!(self.store.len() == P::MAX.as_usize() + 1, "split: block not at capacity");
        let right = self.store.split(at);
        let mut translator = self.translator.clone();
        let at = P::from_usize(at);
        //we want the right half to maintain its internal pointers integrity.
        //splitting at=5 where phys 5 mapped to phys X before, now it maps to 0, so we must 
        //update inner offset to at. 
        //p2v = ((p+io)<<shl + oo).rot_l(rot), v2p = (v.rot_r(rot) - oo) >> shl - io. 
        translator.set_inner_offset(P::ZERO.wrapping_sub(at));
        Self {
            store:      right,
            translator,
            _strategy:  PhantomData,
            _phantom:   PhantomData,
        }
    }

    ///split [at,len) into a new block, cloning the translator. 
    ///precondition: len == P::MAX.as_usize() + 1 (block full).
    ///caller guarantees no nodes present in right point to nodes in left. 
    ///l_odds and r_odds let the caller designate if the spread should land elements on even or odd 
    ///slots in each half
    pub(crate) fn split_and_rotate_block(&mut self, at : usize) -> Self {
        let mut r = self.split_block(at);
        r.store.spread(1);
        r.translator.set_rotation((r.translator.rotation()+1) % P::BIT_WIDTH as u32);
        self.store.spread(0);
        self.translator.set_rotation((self.translator.rotation()+1) % P::BIT_WIDTH as u32);
        return r;
    }
}

//one generic BlockMutTrait impl for Uniform<O>: per-ordering differences live in the
//AllocStrat consts (SPREAD_OFFSET, GROW_*) read by on_grow/spread, so the body is
//ordering-agnostic. the `const` cap-assert monomorphizes per O under the bound.

impl<'a, T, P, O: Ordering, const CAP: usize> BlockMutTrait<'a>
    for RawBlock<'a, T, P, Uniform<O>, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
    Uniform<O>: AllocStrat<P>,
{
    type A = Uniform<O>;
    type CursorMut<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor mut Self>
    where 'a: 'cursor;

    fn new() -> Self {
        const {
            assert!(
                CAP <= <Uniform<O> as AllocStrat<P>>::CAP_LIMIT,
                "CAP exceeds Uniform::CAP_LIMIT"
            );
        }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<P> {
        &mut self.translator
    }
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }

    fn split(&mut self, at : usize) -> Self {
        self.split_block(at)
    }

    fn split_and_rotate(&mut self, at : usize) -> Self {
        self.split_and_rotate_block(Self::A::SPREAD_OFFSET)
    }

    fn try_insert_back(&mut self, v: T) -> Result<usize, T> {
        Err(v)
    }
    fn try_insert_front(&mut self, v: T) -> Result<usize, T> {
        Err(v)
    }

        ///find free slot or make space if possible. dir is logical (true=after);
    ///REVERSED strategies flip it to phys. `pos`/`pin` are physical.
    fn find_slot(
        &mut self,
        mut pos: usize,
        dir: bool,
        mut pin: Option<usize>,
    ) -> FoundSlot {
        let mut found = FoundSlot{grew:None,slide:None};
        let shift = self.translator().shift();
        if self.occupied() * 3 > self.len() * 4 && shift > 0 {
            if let Ok(g) = self.grow_and_spread() {
                g.fix_p(&mut pos);
                pin.as_mut().map(|x| g.fix_p(x));
                found.grew=Some(g);
            }
        }
        let dir = dir ^ Self::A::REVERSED;
        if let Some(ns) = self.store().find_slot(pos, dir, Self::A::INSERT_BUDGET, pin) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == self.max_capacity() {
            return found;
        }
        //this will never happen after a prior grow in the same lookup unless INSERT_BUDGET is 0 which would make no sense. 
        if let Ok(g) = self.grow_and_spread() {
            g.fix_p(&mut pos);
            pin.as_mut().map(|x| g.fix_p(x));
            found.grew=Some(g);
        }
        found.slide = self.store().find_slot(pos, dir, self.len(), pin);
        found
    }
}

impl<'a, T, P, O: Ordering, const CAP: usize> BlockMutTrait<'a>
    for RawBlock<'a, T, P, Pluripotent<O>, DequeStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Pluripotent<O>;
    type CursorMut<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor mut Self>
    where 'a: 'cursor;

    fn new() -> Self {
        const {
            assert!(
                CAP <= <Pluripotent<O> as AllocStrat<P>>::CAP_LIMIT,
                "CAP exceeds Pluripotent::CAP_LIMIT"
            );
        }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut DequeStore<T, CAP> {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<P> {
        &mut self.translator
    }
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }

    fn split(&mut self, at : usize) -> Self {
        self.split_block(at)
    }

    fn split_and_rotate(&mut self, at : usize) -> Self {
        self.split_and_rotate_block(at)
    }

    ///dense append into the free half.
    fn try_insert_back(&mut self, v: T) -> Result<usize, T> {
        if self.len() < self.max_capacity() {
            let p = self.store.push_back(v);
            return Ok(p);
        }
        Err(v)
    }

    ///dense append into the free half.
    fn try_insert_front(&mut self, v: T) -> Result<usize, T> {
        if self.len() < self.max_capacity() {
            //on_push_front bumps inner_offset to cancel the phys shift push_front causes.
            self.store.push_front(v);
            Self::A::on_push_front(self.translator_mut());
            return Ok(0);
        }
        Err(v)
    }

}

///Append: dense push_back (front=low). shift 0, offset=-K (low K reserved for the
///rare prepend). one None per BUDGET pushes stocks mid-insert gaps.
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a>
    for RawBlock<'a, T, P, Append, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Append;
    type CursorMut<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor mut Self>
    where 'a: 'cursor;

    fn new() -> Self {
        const {
            assert!(
                CAP <= <Append as AllocStrat<P>>::CAP_LIMIT,
                "CAP exceeds Append::CAP_LIMIT"
            );
        }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<P> {
        &mut self.translator
    }
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }

    fn split(&mut self, at : usize) -> Self {
        self.split_block(at)
    }
    fn split_and_rotate(&mut self, at : usize) -> Self {
        self.split_and_rotate_block(at)
    }

    ///hot: dense push_back; every BUDGET-th push stocks a None gap for mid-inserts.
    fn try_insert_back(&mut self, v: T) -> Result<usize, T> {
        let occ = self.occupied();
        let pad = occ != 0 && occ % <Append as AllocStrat<P>>::INSERT_BUDGET == 0;
        if self.len() + 1 + pad as usize > self.max_capacity() {
            return Err(v);
        }
        if pad {
            self.store_mut().grow_back(1);
        }
        let p = self.store_mut().push_back(v);
        Ok(p)
    }

    ///cold: push_front into the reserved low range; on_push_front bumps inner_offset
    ///to cancel the phys shift. refuses once the K reservation is spent (offset==MIN).
    fn try_insert_front(&mut self, v: T) -> Result<usize, T> {
        if self.translator().inner_offset() == P::MIN {
            return Err(v);
        }
        self.store_mut().push_front(v);
        Self::A::on_push_front(self.translator_mut());
        Ok(0)
    }

}

///Prepend: Append's layout reversed — push_back is the hot front insert (front=high),
///iteration is high→low, find_slot dir flipped (REVERSED). cold push_front hits the
///reserved low range as the back.
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a>
    for RawBlock<'a, T, P, Prepend, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Prepend;
    type CursorMut<'cursor>
        = BlockCursor<'a, 'cursor, Self, &'cursor mut Self>
    where 'a: 'cursor;

    fn new() -> Self {
        const {
            assert!(
                CAP <= <Prepend as AllocStrat<P>>::CAP_LIMIT,
                "CAP exceeds Prepend::CAP_LIMIT"
            );
        }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<P> {
        &mut self.translator
    }
    fn cursor_mut<'cursor>(&'cursor mut self) -> Self::CursorMut<'cursor>
    where 'a: 'cursor {
        BlockCursor::new(self)
    }

    fn split(&mut self, at : usize) -> Self {
        self.split_block(at)
    }
    fn split_and_rotate(&mut self, at : usize) -> Self {
        self.split_and_rotate_block(at)
    }

    ///hot: push_back (front=high); every BUDGET-th push stocks a None gap.
    fn try_insert_front(&mut self, v: T) -> Result<usize, T> {
        let occ = self.occupied();
        let pad = occ != 0 && occ % <Prepend as AllocStrat<P>>::INSERT_BUDGET == 0;
        if self.len() + 1 + pad as usize > self.max_capacity() {
            return Err(v);
        }
        if pad {
            self.store_mut().grow_back(1);
        }
        let p = self.store_mut().push_back(v);
        Ok(p)
    }

    ///cold: physical push_front into the reserved low range (the back, for Prepend);
    ///on_push_front bumps inner_offset to cancel the phys shift. refuses at offset==MIN.
    fn try_insert_back(&mut self, v: T) -> Result<usize, T> {
        if self.translator().inner_offset() == P::MIN {
            return Err(v);
        }
        self.store_mut().push_front(v);
        Self::A::on_push_front(self.translator_mut());
        Ok(0)
    }

}
#[cfg(test)]
#[path = "tests/block.rs"]
mod tests;
