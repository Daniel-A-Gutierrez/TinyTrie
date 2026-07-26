use crate::{index::*, store::{Cursor, MinSlide, Store}, translator::{AddressTranslator, Translator}};
use std::{cmp::Ordering::{Equal, Greater, Less}, collections::VecDeque, marker::PhantomData, ops::Range};
pub trait Ordering {}
///only exposes append, guaranteeing elements stay in insert-order.
pub struct Insert;
///user maintains ordering and handles ptr updating.
pub struct Manual {}
impl Ordering for Insert {}
impl Ordering for Manual {}
///max cap is bounded by P::Half::MAX. cannot exhaust address space.
pub trait AllocStrat {}
pub struct Pluripotent {}
pub struct Uniform {}
impl AllocStrat for Pluripotent {}
impl AllocStrat for Uniform {}

///read-only block surface: no insert/remove/reorg. get/iter/cursor/first/last/translate.
///ordered block and tree block build on this. T is the stored element type (generic, like Block).
pub trait BlockBase<'a, T: Sized + 'a, P: BlockIndex> {
    fn get(&self, ptr: P) -> &T;
    fn get_mut(&mut self, ptr: P) -> &mut T;
    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr(&self) -> Option<P>;
    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr(&self) -> Option<P>;
    fn v2p(&self, virt: P) -> usize;
    fn p2v(&self, phys: usize) -> P;
    fn vdist(&self, v1: P, v2: P) -> usize;
    fn occupied(&self) -> usize;
    fn len(&self) -> usize;
    fn cap(&self) -> usize;
    fn max_capacity(&self) -> usize;
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b,
        'a: 'b;
    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where
        T: 'b,
        'a: 'b;
}

pub struct Block<'a, T, P, O, A, S,>
where
    T: Sized + 'a,
    P : BlockIndex,
    O: Ordering,
    A : AllocStrat,
    S: Store<'a,T>,
{
    _ordering: PhantomData<O>,
    _strategy: PhantomData<A>,
    store:     S,
    translator: Translator<P>,
    _phantom:  PhantomData<&'a T>,
}

struct OpenSlot(usize);

impl<'a, T, P, O, S> Block<'a,T, P, O, Uniform, S>
where
    T: Sized + 'a,
    P : BlockIndex,
    O: Ordering,
    S: Store<'a,T>,
{
    fn new() -> Self {
        //uniform (random): full address range. log2(cap)+shift = bit_width;
        //cap 0->2^W (= P::MAX+1), shift W->0. offset = 1<<(shift-1) = midpoint of
        //the addressable space (2^W); constant through growth (spread only drops shift).
        let w = P::bit_width() as u32;
        debug_assert!(S::max_capacity() == 1usize << w, "uniform: store MAX_CAP must be 1<<bit_width");
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: S::new(),
            translator: Translator::new(P::from_usize(1usize << (w - 1)), w, 0),
            _phantom: PhantomData,
        }
    }
    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get(&self, ptr: P) -> &T { self.store.get(self.translator.v2p(ptr)) }
    fn get_mut(&mut self, ptr: P) -> &mut T {
        let phys = self.translator.v2p(ptr);
        self.store.get_mut(phys)
    }
    ///slide the None at `from` to `to`; returns `to`. `from==to` => no slide. `pin`, if set, is a
    ///slot whose element must not move.
    ///Precondition: `to != pin` (a pinned `to` can't open).
    fn slide_none(&mut self, ms: MinSlide, pin: Option<P>) -> OpenSlot {
        let pin = pin.map(|p| self.translator.v2p(p));
        OpenSlot(self.store.slide_none(ms, pin))
    }
    ///find the smallest slide of elements to free up the space before or after pos , determined by POS
    ///`pin` is a slot the search must not cross; `pos==pin` restricts to the `DIR` side only.
    fn find_slot<const DIR: bool>(&self, pos: P, budget: usize, pin: Option<P>) -> Option<MinSlide> {
        let pos = self.translator.v2p(pos);
        let pin = pin.map(|p| self.translator.v2p(p));
        self.store.find_slot::<DIR>(pos, budget, pin)
    }
    ///the space at i must be None or panic.
    fn insert(&mut self, v: T, i: OpenSlot) { self.store.insert(v, i.0) }
    ///number of Some slots
    fn occupied(&self) -> usize { self.store.occupied() }
    ///number of None + Some slots
    fn len(&self) -> usize { self.store.len() }
    ///size of None + Some + MaybeUninit slots
    fn cap(&self) -> usize { self.store.cap() }
    fn remove(&mut self, i: P) -> T { self.store.remove(self.translator.v2p(i)) }
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split(at);
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: right,
            translator: Translator::new(self.translator.offset(), self.translator.shift(), self.translator.rotation()),
            _phantom: PhantomData,
        }
    }
    ///split at `at` and odds-gap both halves: self keeps left half at odd slots 2p+1 (even None),
    ///new same-cap store gets right half (reindexed from 0) at odd slots 2k+1. old right-half slots overwritten.
    ///actually split and rotate is only valid at 1<<(bit_width/2) .
    fn split_and_rotate(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split_and_rotate(at);
        //rotation is the split remap primitive: rotate left 1 per split.
        self.translator.set_rotation(self.translator.rotation() + 1);
        let new_t = Translator::new(self.translator.offset(), self.translator.shift(), self.translator.rotation());
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: right,
            translator: new_t,
            _phantom: PhantomData,
        }
    }
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b
    { self.store.iter() }
    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where
        T: 'b
    { self.store.cursor() }
    //the maximum capacity of the store type.
    fn max_capacity(&self) -> usize { S::max_capacity() }
    ///virtual address to physical slot
    fn v2p(&self, virt: P) -> usize { self.translator.v2p(virt) }
    ///physical slot to virtual address
    fn p2v(&self, phys: usize) -> P { self.translator.p2v(phys) }
    ///physical absolute distance between two vptrs;
    fn vdist(&self, v1: P, v2: P) -> usize { self.translator.vdist(v1, v2) }
}

//strategy-agnostic read surface: hits store+translator directly, valid for any A.
impl<'a, T, P, O, A, S> BlockBase<'a, T, P> for Block<'a, T, P, O, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    O: Ordering,
    A: AllocStrat,
    S: Store<'a, T>,
{
    fn get(&self, ptr: P) -> &T { self.store.get(self.translator.v2p(ptr)) }
    fn get_mut(&mut self, ptr: P) -> &mut T { self.store.get_mut(self.translator.v2p(ptr)) }
    fn first_vaddr(&self) -> Option<P> {
        self.store.cursor().first().map(|p| self.translator.p2v(p))
    }
    fn last_vaddr(&self) -> Option<P> {
        self.store.cursor().last().map(|p| self.translator.p2v(p))
    }
    fn v2p(&self, virt: P) -> usize { self.translator.v2p(virt) }
    fn p2v(&self, phys: usize) -> P { self.translator.p2v(phys) }
    fn vdist(&self, v1: P, v2: P) -> usize { self.translator.vdist(v1, v2) }
    fn occupied(&self) -> usize { self.store.occupied() }
    fn len(&self) -> usize { self.store.len() }
    fn cap(&self) -> usize { self.store.cap() }
    fn max_capacity(&self) -> usize { S::max_capacity() }
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b,
        'a: 'b,
    {
        self.store.iter()
    }
    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where
        T: 'b,
        'a: 'b,
    {
        self.store.cursor()
    }
}

impl<'a, T, P, O, S> Block<'a,T, P, O, Pluripotent, S>
where
    T: Sized + 'a,
    P : BlockIndex,
    O: Ordering,
    S: Store<'a,T>,
{
    fn new() -> Self {
        //pluripotent: half the address range. log2(cap)+shift = bit_width/2;
        //cap 0->2^(W/2) (= Half::MAX+1), shift W/2->0. leaves W/2 address bits
        //free for graduation into append/random. offset = 1<<(shift-1) = midpoint
        //of the pluripotent addressable space (2^(W/2)); constant through growth.
        let w = P::Half::bit_width() as u32;
        debug_assert!(S::max_capacity() == 1usize << w, "pluripotent: store MAX_CAP must be 1<<(bit_width/2)");
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: S::new(),
            translator: Translator::new(P::from_usize(1usize << P::bit_width()-1), w, 0),
            _phantom: PhantomData,
        }
    }
    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get(&self, ptr: P) -> &T { self.store.get(self.translator.v2p(ptr)) }
    fn get_mut(&mut self, ptr: P) -> &mut T {
        let phys = self.translator.v2p(ptr);
        self.store.get_mut(phys)
    }
    ///slide the None at `from` to `to`; returns `to`. `from==to` => no slide. `pin`, if set, is a
    ///slot whose element must not move.
    ///Precondition: `to != pin` (a pinned `to` can't open).
    fn slide_none(&mut self, ms: MinSlide, pin: Option<P>) -> OpenSlot {
        let pin = pin.map(|p| self.translator.v2p(p));
        OpenSlot(self.store.slide_none(ms, pin))
    }
    ///find the smallest slide of elements to free up the space before or after pos , determined by POS
    ///`pin` is a slot the search must not cross; `pos==pin` restricts to the `DIR` side only.
    fn find_slot<const DIR: bool>(&self, pos: P, budget: usize, pin: Option<P>) -> Option<MinSlide> {
        let pos = self.translator.v2p(pos);
        let pin = pin.map(|p| self.translator.v2p(p));
        self.store.find_slot::<DIR>(pos, budget, pin)
    }
    ///the space at i must be None or panic.
    fn insert(&mut self, v: T, i: OpenSlot) { self.store.insert(v, i.0) }
    ///spread (doubling phys slots, element i -> 2i) keeps existing vptrs stable:
    ///v2p gains a >>1, so shift -= 1 cancels the phys doubling. address-stable growth.
    fn push_front(&mut self, v : T) -> P {
        if self.store.len() > 0 && self.store.len() == self.store.cap() {
            self.store.spread();
            self.translator.set_shift(self.translator.shift() - 1);
        }
        self.store.push_front(v);
        //cancel VecDeque::push_front's physical shift: virt_offset += 1 << addr_shift.
        //NOT += 1 — stable for all addr_shift (regression-locked).
        let bump = P::from_usize(1).wrapping_shl(self.translator.shift());
        self.translator.set_offset(self.translator.offset().wrapping_add(bump));
        self.translator.p2v(0)
    }
    fn push_back(&mut self, v: T) -> P {
        if self.store.len() > 0 && self.store.len() == self.store.cap() {
            self.store.spread();
            self.translator.set_shift(self.translator.shift() - 1);
        }
        let phys = self.store.push_back(v);
        self.translator.p2v(phys)
    }
    ///number of Some slots
    fn occupied(&self) -> usize { self.store.occupied() }
    ///number of None + Some slots
    fn len(&self) -> usize { self.store.len() }
    ///size of None + Some + MaybeUninit slots
    fn cap(&self) -> usize { self.store.cap() }
    fn remove(&mut self, i: P) -> T { self.store.remove(self.translator.v2p(i)) }
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split(at);
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: right,
            translator: Translator::new(self.translator.offset(), self.translator.shift(), self.translator.rotation()),
            _phantom: PhantomData,
        }
    }
    ///split at `at` and odds-gap both halves: self keeps left half at odd slots 2p+1 (even None),
    ///new same-cap store gets right half (reindexed from 0) at odd slots 2k+1. old right-half slots overwritten.
    ///actually split and rotate is only valid at 1<<(bit_width/2) .
    fn split_and_rotate(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split_and_rotate(at);
        self.translator.set_rotation(self.translator.rotation() + 1);
        let new_t = Translator::new(self.translator.offset(), self.translator.shift(), self.translator.rotation());
        Self {
            _ordering: PhantomData,
            _strategy: PhantomData,
            store: right,
            translator: new_t,
            _phantom: PhantomData,
        }
    }
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b
    { self.store.iter() }
    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where
        T: 'b
    { self.store.cursor() }
    //the maximum capacity of the store type.
    fn max_capacity(&self) -> usize { S::max_capacity() }
    ///virtual address to physical slot
    fn v2p(&self, virt: P) -> usize { self.translator.v2p(virt) }
    ///physical slot to virtual address
    fn p2v(&self, phys: usize) -> P { self.translator.p2v(phys) }
    ///physical absolute distance between two vptrs;
    fn vdist(&self, v1: P, v2: P) -> usize { self.translator.vdist(v1, v2) }
}