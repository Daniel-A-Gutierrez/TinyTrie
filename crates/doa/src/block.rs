use crate::{index::*, store::{Cursor, NoneSlide, Store, VecStore, DequeStore}, translator::{AddressTranslator, Translator}};
use std::marker::PhantomData;


pub trait AllocStrat<P: BlockIndex>: 'static {
    ///initial shift for an empty block. Uniform = P::BIT_WIDTH (full range);
    ///Pluripotent = P::Half::BIT_WIDTH; Append/Prepend = 0 (dense).
    const INIT_SHIFT: u32;

    ///initial offset (as usize; new_block wraps into P). Anchor so growth has
    ///headroom on the non-dominant side. Uniform/Pluripotent = MIDPOINT;
    ///Append/Prepend = -K (K = Half-range) so the low K addresses stay free.
    const INIT_OFFSET: usize;

    ///budget for the find_slot walk before triggering a spread. Append/Prepend
    ///also use this as the None-padding period (one None per BUDGET pushes),
    ///so a mid-insert always reaches a gap within budget.
    const INSERT_BUDGET: usize;

    ///max legal store CAP.
    const CAP_LIMIT: usize;

    ///logical direction reversed (front = high end). Prepend only.
    const REVERSED: bool;
}

pub struct Uniform;
pub struct Pluripotent;
pub struct Append;
pub struct Prepend;

impl<P: BlockIndex> AllocStrat<P> for Uniform {
    const INIT_SHIFT: u32 = P::BIT_WIDTH as u32;
    const INIT_OFFSET: usize = 1 << (P::BIT_WIDTH - 1);
    const INSERT_BUDGET: usize = P::BIT_WIDTH as usize;
    const CAP_LIMIT: usize = 1 << P::BIT_WIDTH;
    const REVERSED: bool = false;
}

impl<P: BlockIndex> AllocStrat<P> for Pluripotent {
    const INIT_SHIFT: u32 = P::Half::BIT_WIDTH as u32 - 1;
    const INIT_OFFSET: usize = 1 << (P::BIT_WIDTH - 1);
    const INSERT_BUDGET: usize = P::Half::BIT_WIDTH as usize;
    const CAP_LIMIT: usize = 1 << P::Half::BIT_WIDTH;
    const REVERSED: bool = false;
}

impl<P: BlockIndex> AllocStrat<P> for Append {
    const INIT_SHIFT: u32 = 0;
    const INIT_OFFSET: usize = (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH);
    const INSERT_BUDGET: usize = 16;
    const CAP_LIMIT: usize = (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH);
    const REVERSED: bool = false;
}

impl<P: BlockIndex> AllocStrat<P> for Prepend {
    const INIT_SHIFT: u32 = 0;
    const INIT_OFFSET: usize = (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH);
    const INSERT_BUDGET: usize = 16;
    const CAP_LIMIT: usize = (1 << P::BIT_WIDTH) - (1 << P::Half::BIT_WIDTH);
    const REVERSED: bool = true;
}

///read-only block surface
pub trait BlockTrait<'a, T: Sized + 'a, P: BlockIndex, S: Store<'a, T> + 'a>: 'a {

    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b;

    fn translator<'b>(&'b self) -> &'b Translator<P>;

    fn get<'b>(&'b self, ptr: P) -> &'b T
    where 'a: 'b {
        self.store().get(self.translator().v2p(ptr))
    }

    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<P>
    where 'a: 'b {
        self.store().cursor().first().map(|p| self.translator().p2v(p))
    }

    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<P>
    where 'a: 'b {
        self.store().cursor().last().map(|p| self.translator().p2v(p))
    }

    fn v2p<'b>(&'b self, virt: P) -> usize {
        self.translator().v2p(virt)
    }

    fn p2v<'b>(&'b self, phys: usize) -> P {
        self.translator().p2v(phys)
    }

    fn vdist<'b>(&'b self, v1: P, v2: P) -> usize {
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
        S::max_capacity()
    }

    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where 'a: 'b {
        self.store().iter()
    }

    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where 'a: 'b {
        self.store().cursor()
    }
}

///mutation surface. blocks of different alloc strats implement a common interface but failures must be handled at runtime. 
pub trait BlockMutTrait<'a, T: Sized + 'a, P: BlockIndex, A : AllocStrat<P>, S: Store<'a, T> + 'a>:
    BlockTrait<'a, T, P, S>
{
    fn new() -> Self;
    fn store_mut(&mut self) -> &mut S;

    fn translator_mut(&mut self) -> &mut Translator<P>;

    fn get_mut<'b>(&'b mut self, ptr: P) -> &'b mut T where 'a:'b{
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }

    ///slide the None `ms.from` -> `ms.to`; returns the opened slot. `pin` must not move.
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<P>) -> OpenSlot {
        let pin = {
            let tr = self.translator();
            pin.map(|p| tr.v2p(p))
        };
        OpenSlot(self.store_mut().slide_none(ms, pin))
    }

    ///first insert into an empty block (lands at phys 0 = the anchor vaddr).
    fn insert_root(&mut self, v: T) -> P {
        debug_assert!(self.store().len() == 0, "insert_first: block not empty");
        self.store_mut().push_back(v);
        self.translator().p2v(0)
    }

    ///manually grow + spread; fails if shift==0 or would exceed max capacity.
    fn grow_and_spread(&mut self) -> Result<(), ()> {
        let shift = self.translator().shift();
        if shift == 0 {
            return Err(());
        }
        if self.store().len() * 2 > S::max_capacity() {
            return Err(());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread();
        Ok(())
    }

    ///find free slot or make space if possible. dir is logical (true=after);
    ///REVERSED strategies flip it to phys.
    fn find_slot(&mut self, pos: P, dir: bool, pin: Option<P>) -> Option<NoneSlide> {
        let dir = dir ^ A::REVERSED;
        let pp = self.translator().v2p(pos);
        let pinp = pin.map(|p| self.translator().v2p(p));
        if let Some(ms) = self.store().find_slot(pp, dir, A::INSERT_BUDGET, pinp) {
            return Some(ms);
        }
        if self.len()==self.max_capacity() { return None }
        let _ = self.grow_and_spread();
        //spread shifted phys (i->2i) and halved shift; vaddrs are stable, so re-translate.
        let pp = self.translator().v2p(pos);
        let pinp = pin.map(|p| self.translator().v2p(p));
        self.store().find_slot(pp, dir, self.len(), pinp)
    }

    ///place `v` at the opened slot. returns its vaddr.
    fn insert(&mut self, v: T, slot: OpenSlot) -> P ;

    fn remove(&mut self, ptr: P) -> T {
        let p = self.translator().v2p(ptr);
        self.store_mut().remove(p)
    }

    //none of the split stuff is really in use or correct or working.
    
    ///split at P::MIDPOINT: [MIDPOINT,len) move into a new block; self keeps [0,MIDPOINT).
    ///precondition: len == P::MAX.as_usize() + 1 (block full).
    fn split(&mut self) -> Self;

    ///split at P::MIDPOINT + odds-gap both halves (rotation is the split remap primitive).
    ///precondition: len == P::MAX.as_usize() + 1.
    fn split_and_rotate(&mut self) -> Self;

    ///failure is a signal to use a different block or block type.
    ///will not move elements
    fn try_insert_back(&mut self, v: T) -> Result<P,T>;

    ///failure is a signal to use a different block or block type.
    ///will not move elements
    fn try_insert_front(&mut self, v: T) -> Result<P,T>;
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
pub(crate) enum DirIter<F, R> { Fwd(F), Rev(R) }

impl<F: Iterator, R: Iterator<Item = F::Item>> Iterator for DirIter<F, R> {
    type Item = F::Item;
    #[inline]
    fn next(&mut self) -> Option<F::Item> {
        match self { Self::Fwd(i) => i.next(), Self::Rev(i) => i.next() }
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self { Self::Fwd(i) => i.size_hint(), Self::Rev(i) => i.size_hint() }
    }
}

impl<F: ExactSizeIterator, R: ExactSizeIterator<Item = F::Item>> ExactSizeIterator
    for DirIter<F, R>
{}

impl<'a, T, P, A, S> BlockTrait<'a, T, P, S> for RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
{

    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        &self.store
    }

    fn translator<'b>(&'b self) -> &'b Translator<P> {
        &self.translator
    }

    ///REVERSED strategies iterate high→low (front at the back).
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where 'a: 'b {
        let it = self.store().iter();
        if A::REVERSED { DirIter::Rev(it.rev()) } else { DirIter::Fwd(it) }
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
    ///empty block: offset = A::INIT_OFFSET (anchor w/ headroom on the non-dominant
    ///side); shift = A::INIT_SHIFT; rotation 0.
    pub(crate) fn new_block() -> Self {
        let shift = A::INIT_SHIFT;
        debug_assert!(
            S::max_capacity() <= A::CAP_LIMIT,
            "store MAX_CAP exceeds strategy CAP_LIMIT"
        );
        Self {
            _strategy:  PhantomData,
            store:      S::new(),
            translator: Translator::new(P::from_usize(A::INIT_OFFSET), shift, 0),
            _phantom:   PhantomData,
        }
    }

    ///split at P::MIDPOINT (virtual): [MIDPOINT,len) move into a new block; self keeps
    ///[0,MIDPOINT). precondition: len == P::MAX.as_usize() + 1 (block full).
    pub(crate) fn split_block(&mut self) -> Self {
        debug_assert!(self.store.len() == P::MAX.as_usize() + 1, "split: block not full");
        let at = self.translator.v2p(P::MIDPOINT);
        let right = self.store.split(at);
        Self {
            _strategy:  PhantomData,
            store:      right,
            translator: Translator::new(
                self.translator.offset(),
                self.translator.shift(),
                self.translator.rotation(),
            ),
            _phantom:   PhantomData,
        }
    }

    ///split at P::MIDPOINT + odds-gap both halves (rotation is the split remap primitive).
    ///precondition: len == P::MAX().as_usize() + 1.
    pub(crate) fn split_and_rotate_block(&mut self) -> Self {
        debug_assert!(self.store.len() == P::MAX.as_usize() + 1, "split_and_rotate: block not full");
        let at = P::MIDPOINT.as_usize();
        let right = self.store.split_and_rotate(at);
        self.translator.set_rotation(self.translator.rotation() + 1);
        Self {
            _strategy:  PhantomData,
            store:      right,
            translator: Translator::new(
                self.translator.offset(),
                self.translator.shift(),
                self.translator.rotation(),
            ),
            _phantom:   PhantomData,
        }
    }
}

impl<'a, T, P, const CAP : usize> BlockMutTrait<'a, T, P, Uniform, VecStore<T, CAP>>
for RawBlock<'a, T, P, Uniform, VecStore<T,CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    fn new() -> Self {
        const { assert!(CAP <= <Uniform as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Uniform::CAP_LIMIT"); }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> { &mut self.store }
    fn translator_mut(&mut self) -> &mut Translator<P> { &mut self.translator }

    fn split(&mut self) -> Self { self.split_block() }
    fn split_and_rotate(&mut self) -> Self { self.split_and_rotate_block() }

    ///dense append into the free half.
    fn try_insert_back(&mut self, v: T) -> Result<P,T> {
        return Err(v);
    }

    ///dense append into the free half.
    fn try_insert_front(&mut self, v: T) -> Result<P,T> {
        return Err(v)
    }

    fn insert(&mut self, v: T, slot: OpenSlot) -> P {
        //vaddr is stable across the spread below (i->2i, shift-1); compute it first.
        let vaddr = self.translator().p2v(slot.0);
        self.store_mut().insert(v, slot.0);
        let shift = self.translator().shift();
        if self.occupied() * 3 > self.len() * 4 && shift > 0 {
            self.translator_mut().set_shift(shift - 1);
            self.store_mut().spread();
        }
        vaddr
    }
}

impl<'a, T, P, const CAP : usize> BlockMutTrait<'a, T, P, Pluripotent, DequeStore<T,CAP>>
for RawBlock<'a, T, P, Pluripotent, DequeStore<T,CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    fn new() -> Self {
        const { assert!(CAP <= <Pluripotent as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Pluripotent::CAP_LIMIT"); }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut DequeStore<T, CAP> { &mut self.store }
    fn translator_mut(&mut self) -> &mut Translator<P> { &mut self.translator }

    fn split(&mut self) -> Self { self.split_block() }
    fn split_and_rotate(&mut self) -> Self { self.split_and_rotate_block() }

    ///dense append into the free half.
    fn try_insert_back(&mut self, v: T) -> Result<P,T> {
        if self.len() < self.max_capacity() {
            let p = self.store.push_back(v);
            return Ok(self.translator.p2v(p)); 
        }
        return Err(v);
    }

    ///dense append into the free half.
    fn try_insert_front(&mut self, v: T) -> Result<P,T> {
        if self.len() < self.max_capacity() {
            //+1<<shift cancels the phys shift push_front causes (stable for all shift,
            //not just 0 — a regression test locks it). NOT `+ P::ONE`.
            let bump = P::from_usize(1usize << self.translator.shift());
            let new_offset = self.translator.offset() + bump;
            self.store.push_front(v);
            self.translator_mut().set_offset(new_offset);
            return Ok(self.translator.p2v(0));
        }
        return Err(v);
    }

    fn insert(&mut self, v: T, slot: OpenSlot) -> P {
        self.store_mut().insert(v, slot.0);
        self.translator().p2v(slot.0)
    }
}

///Append: dense push_back (front=low). shift 0, offset=-K (low K reserved for the
///rare prepend). one None per BUDGET pushes stocks mid-insert gaps.
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a, T, P, Append, VecStore<T, CAP>>
for RawBlock<'a, T, P, Append, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    fn new() -> Self {
        const { assert!(CAP <= <Append as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Append::CAP_LIMIT"); }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> { &mut self.store }
    fn translator_mut(&mut self) -> &mut Translator<P> { &mut self.translator }

    fn split(&mut self) -> Self { self.split_block() }
    fn split_and_rotate(&mut self) -> Self { self.split_and_rotate_block() }

    ///hot: dense push_back; every BUDGET-th push stocks a None gap for mid-inserts.
    fn try_insert_back(&mut self, v: T) -> Result<P, T> {
        let occ = self.occupied();
        let pad = occ != 0 && occ % <Append as AllocStrat<P>>::INSERT_BUDGET == 0;
        if self.len() + 1 + pad as usize > self.max_capacity() { return Err(v); }
        if pad { self.store_mut().grow_back(1); }
        let p = self.store_mut().push_back(v);
        Ok(self.translator().p2v(p))
    }

    ///cold: push_front into the reserved low range; offset+1 cancels the phys shift
    ///so existing addrs stay stable. refuses once the K reservation is spent (offset
    ///wraps to MIN). wrapping_add — offset passes through MAX before reaching MIN.
    fn try_insert_front(&mut self, v: T) -> Result<P, T> {
        let offset = self.translator().offset();
        if offset == P::MIN { return Err(v); }
        self.store_mut().push_front(v);
        self.translator_mut().set_offset(offset.wrapping_add(P::ONE));
        Ok(self.translator().p2v(0))
    }

    fn insert(&mut self, v: T, slot: OpenSlot) -> P {
        self.store_mut().insert(v, slot.0);
        self.translator().p2v(slot.0)
    }
}

///Prepend: Append's layout reversed — push_back is the hot front insert (front=high),
///iteration is high→low, find_slot dir flipped (REVERSED). cold push_front hits the
///reserved low range as the back.
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a, T, P, Prepend, VecStore<T, CAP>>
for RawBlock<'a, T, P, Prepend, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    fn new() -> Self {
        const { assert!(CAP <= <Prepend as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Prepend::CAP_LIMIT"); }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> { &mut self.store }
    fn translator_mut(&mut self) -> &mut Translator<P> { &mut self.translator }

    fn split(&mut self) -> Self { self.split_block() }
    fn split_and_rotate(&mut self) -> Self { self.split_and_rotate_block() }

    ///hot: push_back (front=high); every BUDGET-th push stocks a None gap.
    fn try_insert_front(&mut self, v: T) -> Result<P, T> {
        let occ = self.occupied();
        let pad = occ != 0 && occ % <Prepend as AllocStrat<P>>::INSERT_BUDGET == 0;
        if self.len() + 1 + pad as usize > self.max_capacity() { return Err(v); }
        if pad { self.store_mut().grow_back(1); }
        let p = self.store_mut().push_back(v);
        Ok(self.translator().p2v(p))
    }

    ///cold: push_front into the reserved low range (the back, for Prepend). wrapping
    ///add — offset passes MAX before reaching MIN (exhaustion).
    fn try_insert_back(&mut self, v: T) -> Result<P, T> {
        let offset = self.translator().offset();
        if offset == P::MIN { return Err(v); }
        self.store_mut().push_front(v);
        self.translator_mut().set_offset(offset.wrapping_add(P::ONE));
        Ok(self.translator().p2v(0))
    }

    fn insert(&mut self, v: T, slot: OpenSlot) -> P {
        self.store_mut().insert(v, slot.0);
        self.translator().p2v(slot.0)
    }
}
#[cfg(test)]
#[path = "tests/block.rs"]
mod tests;
