use crate::{index::*,
            store::{Cursor, MinSlide, Store},
            translator::{AddressTranslator, Translator}};
use std::marker::PhantomData;
///max cap is bounded by P::Half::MAX. cannot exhaust address space.
pub trait AllocStrat: 'static {}
pub struct Pluripotent {}
pub struct Uniform {}
impl AllocStrat for Pluripotent {}
impl AllocStrat for Uniform {}
///read-only block surface: no insert/remove/reorg, no &mut T. get/iter/cursor/
///first/last/translate. RawBlock and TreeBlock both impl this; mutation lives on
///the concrete type so each can uphold its own invariants. T is the stored element.
///S is the store; defaults forward through store()/translator() accessors, so an
///impl only writes those two.
pub trait BlockBase<'a, T: Sized + 'a, P: BlockIndex, S: Store<'a, T> + 'a>: 'a {
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
///raw ordered arena run: owns a store + translator, upholds no structural
///invariant. base storage for TreeBlock (composed, not inherited — TreeBlock
///keeps its inner raw private so callers can't bypass rewiring). the unconstrained
///mutation surface (push/insert/remove/split) returning InsertDelta lives here.
pub struct RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    S: Store<'a, T>,
{
    _strategy:  PhantomData<A>,
    store:      S,
    translator: Translator<P>,
    _phantom:   PhantomData<&'a T>,
}
struct OpenSlot(usize);
//strategy-agnostic: get_mut hands out &mut T freely — RawBlock has no invariants
//to break. (BlockBase deliberately omits get_mut; TreeBlock does too.)
impl<'a, T, P, A, S> RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    S: Store<'a, T>,
{
    fn get_mut(&mut self, ptr: P) -> &mut T {
        let phys = self.translator.v2p(ptr);
        self.store.get_mut(phys)
    }
}
impl<'a, T, P, A, S> BlockBase<'a, T, P, S> for RawBlock<'a, T, P, A, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    S: Store<'a, T> + 'a,
{
    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        &self.store
    }
    fn translator<'b>(&'b self) -> &'b Translator<P> {
        &self.translator
    }
}
impl<'a, T, P, S> RawBlock<'a, T, P, Uniform, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    S: Store<'a, T>,
{
    fn new() -> Self {
        //uniform (random): full address range. log2(cap)+shift = bit_width;
        //cap 0->2^W (= P::MAX+1), shift W->0. offset = 1<<(shift-1) = midpoint of
        //the addressable space (2^W); constant through growth (spread only drops shift).
        let w = P::bit_width() as u32;
        debug_assert!(
            S::max_capacity() == 1usize << w,
            "uniform: store MAX_CAP must be 1<<bit_width"
        );
        Self {
            _strategy:  PhantomData,
            store:      S::new(),
            translator: Translator::new(P::from_usize(1usize << (w - 1)), w, 0),
            _phantom:   PhantomData,
        }
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
    fn find_slot<const DIR: bool>(
        &self,
        pos: P,
        budget: usize,
        pin: Option<P>,
    ) -> Option<MinSlide> {
        let pos = self.translator.v2p(pos);
        let pin = pin.map(|p| self.translator.v2p(p));
        self.store.find_slot::<DIR>(pos, budget, pin)
    }
    ///the space at i must be None or panic.
    fn insert(&mut self, v: T, i: OpenSlot) {
        self.store.insert(v, i.0)
    }
    fn remove(&mut self, i: P) -> T {
        self.store.remove(self.translator.v2p(i))
    }
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
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
    ///split at `at` and odds-gap both halves: self keeps left half at odd slots 2p+1 (even None),
    ///new same-cap store gets right half (reindexed from 0) at odd slots 2k+1. old right-half slots overwritten.
    ///actually split and rotate is only valid at 1<<(bit_width/2) .
    fn split_and_rotate(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split_and_rotate(at);
        //rotation is the split remap primitive: rotate left 1 per split.
        self.translator.set_rotation(self.translator.rotation() + 1);
        let new_t = Translator::new(
            self.translator.offset(),
            self.translator.shift(),
            self.translator.rotation(),
        );
        Self {
            _strategy:  PhantomData,
            store:      right,
            translator: new_t,
            _phantom:   PhantomData,
        }
    }
}
impl<'a, T, P, S> RawBlock<'a, T, P, Pluripotent, S>
where
    T: Sized + 'a,
    P: BlockIndex,
    S: Store<'a, T>,
{
    fn new() -> Self {
        //pluripotent: half the address range. log2(cap)+shift = bit_width/2;
        //cap 0->2^(W/2) (= Half::MAX+1), shift W/2->0. leaves W/2 address bits
        //free for graduation into append/random. offset = 1<<(shift-1) = midpoint
        //of the pluripotent addressable space (2^(W/2)); constant through growth.
        let w = P::Half::bit_width() as u32;
        debug_assert!(
            S::max_capacity() == 1usize << w,
            "pluripotent: store MAX_CAP must be 1<<(bit_width/2)"
        );
        Self {
            _strategy:  PhantomData,
            store:      S::new(),
            translator: Translator::new(P::from_usize(1usize << P::bit_width() - 1), w, 0),
            _phantom:   PhantomData,
        }
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
    fn find_slot<const DIR: bool>(
        &self,
        pos: P,
        budget: usize,
        pin: Option<P>,
    ) -> Option<MinSlide> {
        let pos = self.translator.v2p(pos);
        let pin = pin.map(|p| self.translator.v2p(p));
        self.store.find_slot::<DIR>(pos, budget, pin)
    }
    ///the space at i must be None or panic.
    fn insert(&mut self, v: T, i: OpenSlot) {
        self.store.insert(v, i.0)
    }
    ///spread (doubling phys slots, element i -> 2i) keeps existing vptrs stable:
    ///v2p gains a >>1, so shift -= 1 cancels the phys doubling. address-stable growth.
    fn push_front(&mut self, v: T) -> P {
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
    fn remove(&mut self, i: P) -> T {
        self.store.remove(self.translator.v2p(i))
    }
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
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
    ///split at `at` and odds-gap both halves: self keeps left half at odd slots 2p+1 (even None),
    ///new same-cap store gets right half (reindexed from 0) at odd slots 2k+1. old right-half slots overwritten.
    ///actually split and rotate is only valid at 1<<(bit_width/2) .
    fn split_and_rotate(&mut self, at: P) -> Self {
        let at = self.translator.v2p(at);
        let right = self.store.split_and_rotate(at);
        self.translator.set_rotation(self.translator.rotation() + 1);
        let new_t = Translator::new(
            self.translator.offset(),
            self.translator.shift(),
            self.translator.rotation(),
        );
        Self {
            _strategy:  PhantomData,
            store:      right,
            translator: new_t,
            _phantom:   PhantomData,
        }
    }
}
