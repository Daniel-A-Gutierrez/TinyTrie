use crate::{InOrder, Ordering, PostOrder, PreOrder, index::*, store::{Cursor, DequeStore, NoneSlide, Store, VecStore}, translator::{AddressTranslator, Translator}};
use std::fmt;
use std::fmt::Write as _;
use std::marker::PhantomData;
use crate::alloc_strat::*;



///per-strategy concrete block aliases. `BlockMutTrait` is only impl'd for these four
///(strategy, store) combos, so these are the only `RawBlock` family members that are
///tree-usable as `Inner`. `pub(crate)` because the stores are `pub(crate)`.
pub(crate) type UniformBlock<'a, T, O : Ordering, P, const CAP: usize> =
    RawBlock<'a, T, P, Uniform<O>, VecStore<T, CAP>>;
pub(crate) type PluripotentBlock<'a, T, O : Ordering, P, const CAP: usize> =
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

///read-only block surface. `T`/`P`/`S` are associated (derived from the impl, i.e.
///from the concrete `RawBlock` family member), so the tree tier can recover them as
///`Inner::T`/`Inner::P`/`Inner::S` without restating them as params.
pub trait BlockTrait<'a>: 'a {
    type T: Sized + 'a;
    type P: BlockIndex;
    type S: Store<'a, Self::T> + 'a;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b;

    fn translator<'b>(&'b self) -> &'b Translator<Self::P>;

    fn get<'b>(&'b self, ptr: Self::P) -> &'b Self::T
    where 'a: 'b {
        self.store().get(self.translator().v2p(ptr))
    }

    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'a: 'b {
        self.store().cursor().first().map(|p| self.translator().p2v(p))
    }

    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'a: 'b {
        self.store().cursor().last().map(|p| self.translator().p2v(p))
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

    fn cursor<'b>(&'b self) -> impl Cursor<'b, Self::T> + 'b
    where 'a: 'b {
        self.store().cursor()
    }
}

///mutation surface. blocks of different alloc strats implement a common interface but failures must be handled at runtime.
///`A` (the strategy) is associated; `T`/`P`/`S` come from the `BlockTrait` supertrait.
pub trait BlockMutTrait<'a>: BlockTrait<'a> {
    type A: AllocStrat<Self::P>;

    fn new() -> Self;
    fn store_mut(&mut self) -> &mut Self::S;

    fn translator_mut(&mut self) -> &mut Translator<Self::P>;

    fn get_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::T where 'a:'b{
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }

    ///slide the None `ms.from` -> `ms.to`; returns the opened slot. `pin` must not move.
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<Self::P>) -> OpenSlot {
        let pin = {
            let tr = self.translator();
            pin.map(|p| tr.v2p(p))
        };
        OpenSlot(self.store_mut().slide_none(ms, pin))
    }

    ///first insert into an empty block. grows to `INIT_CAP` Nones and lands the root at
    ///the midpoint phys (`INIT_CAP/2`): for in-order (`INIT_CAP=2`) that's phys 1 — the
    ///physical midpoint of the len-2 block, so the root vaddr is the fixed MIDPOINT.
    fn insert_root(&mut self, v: Self::T) -> Self::P {
        debug_assert!(self.store().len() == 0, "insert_root: block not empty");
        let cap = Self::A::INIT_CAP as usize;
        self.store_mut().grow_back(cap);
        let mid = cap / 2;
        self.store_mut().insert(v, mid);
        self.translator().p2v(mid)
    }

    ///manually grow + spread; fails if shift==0 or would exceed max capacity.
    fn grow_and_spread(&mut self) -> Result<(), ()> {
        let shift = self.translator().shift();
        if shift == 0 {
            return Err(());
        }
        if self.store().len() * 2 > Self::S::max_capacity() {
            return Err(());
        }
        Self::A::on_grow(self.translator_mut());
        self.store_mut().spread(Self::A::SPREAD_OFFSET);
        Ok(())
    }

    ///find free slot or make space if possible. dir is logical (true=after);
    ///REVERSED strategies flip it to phys.
    fn find_slot(&mut self, pos: Self::P, dir: bool, pin: Option<Self::P>) -> Option<NoneSlide> {
        let dir = dir ^ Self::A::REVERSED;
        let pp = self.translator().v2p(pos);
        let pinp = pin.map(|p| self.translator().v2p(p));
        if let Some(ms) = self.store().find_slot(pp, dir, Self::A::INSERT_BUDGET, pinp) {
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
    fn insert(&mut self, v: Self::T, slot: OpenSlot) -> Self::P ;

    fn remove(&mut self, ptr: Self::P) -> Self::T {
        let p = self.translator().v2p(ptr);
        self.store_mut().remove(p)
    }

    ///swap the contents at two vaddrs (translates both to phys, swaps the store slots).
    fn swap(&mut self, a: Self::P, b: Self::P) {
        let pa = self.translator().v2p(a);
        let pb = self.translator().v2p(b);
        self.store_mut().swap(pa, pb);
    }

    ///swap the record at vaddr `target` with the None at `open`. returns the slot freed at
    ///`target`'s old phys and the new vaddr of the record that was at `target` (now at
    ///`open`'s phys). used to relocate a wired node to a new gap (`hop_to_median`) and to
    ///land a new node at a specific vaddr (root promote): the caller inserts into the freed
    ///slot, or reads the returned vaddr to update the node's inbound pointer.
    fn swap_open(&mut self, target: Self::P, open: OpenSlot) -> (OpenSlot, Self::P) {
        let p_target = self.translator().v2p(target);
        self.store_mut().swap(p_target, open.0);
        (OpenSlot(p_target), self.translator().p2v(open.0))
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
    fn try_insert_back(&mut self, v: Self::T) -> Result<Self::P, Self::T>;

    ///failure is a signal to use a different block or block type.
    ///will not move elements
    fn try_insert_front(&mut self, v: Self::T) -> Result<Self::P, Self::T>;
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

///`Debug` view: translator params + physical slot layout (`[i:[child_phys,...], j:X, ...]`).
///the layout is rebuilt from the store cursor — Nones are inserted up to the cursor's
///position so the full sparse physical layout (gaps included) is visible. each Some's
///child vaddrs (`SlotDebug::debug_children`) are mapped to physical slots via `v2p`.
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
        let len = self.store.len();
        let mut buf = String::new();
        buf.push_str("  slots: [");
        let mut cur = self.store.cursor();
        let mut next_some = cur.first();
        let mut phys = 0usize;
        while phys < len {
            if next_some == Some(phys) {
                {
                    let item = cur.current().expect("cursor at Some");
                    if phys > 0 { buf.push_str(", "); }
                    let parts = item.debug_render(tr);
                    let _ = write!(buf, "{phys}:[{}]", parts.join(","));
                }
                cur.next();
                next_some = cur.position();
            } else {
                if phys > 0 { buf.push_str(", "); }
                let _ = write!(buf, "{phys}:X");
            }
            phys += 1;
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
                self.translator.inner_offset(),
                self.translator.outer_offset(),
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
                self.translator.inner_offset(),
                self.translator.outer_offset(),
                self.translator.shift(),
                self.translator.rotation(),
            ),
            _phantom:   PhantomData,
        }
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

    fn new() -> Self {
        const { assert!(CAP <= <Uniform<O> as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Uniform::CAP_LIMIT"); }
        Self::new_block()
    }
    fn store_mut(&mut self) -> &mut VecStore<T, CAP> { &mut self.store }
    fn translator_mut(&mut self) -> &mut Translator<P> { &mut self.translator }

    fn split(&mut self) -> Self { self.split_block() }
    fn split_and_rotate(&mut self) -> Self { self.split_and_rotate_block() }

    fn try_insert_back(&mut self, v: T) -> Result<P, T> { Err(v) }
    fn try_insert_front(&mut self, v: T) -> Result<P, T> { Err(v) }

    fn insert(&mut self, v: T, slot: OpenSlot) -> P {
        //vaddr stable across the spread below (i->2i+SPREAD_OFFSET, on_grow); compute first.
        let vaddr = self.translator().p2v(slot.0);
        self.store_mut().insert(v, slot.0);
        let shift = self.translator().shift();
        if self.occupied() * 3 > self.len() * 4 && shift > 0 {
            Self::A::on_grow(self.translator_mut());
            self.store_mut().spread(Self::A::SPREAD_OFFSET);
        }
        vaddr
    }
}

impl<'a, T, P, O: Ordering, const CAP : usize> BlockMutTrait<'a>
for RawBlock<'a, T, P, Pluripotent<O>, DequeStore<T,CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Pluripotent<O>;

    fn new() -> Self {
        const { assert!(CAP <= <Pluripotent<O> as AllocStrat<P>>::CAP_LIMIT, "CAP exceeds Pluripotent::CAP_LIMIT"); }
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
            //on_push_front bumps inner_offset to cancel the phys shift push_front causes.
            self.store.push_front(v);
            Self::A::on_push_front(self.translator_mut());
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
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a>
for RawBlock<'a, T, P, Append, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Append;

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

    ///cold: push_front into the reserved low range; on_push_front bumps inner_offset
    ///to cancel the phys shift. refuses once the K reservation is spent (offset==MIN).
    fn try_insert_front(&mut self, v: T) -> Result<P, T> {
        if self.translator().inner_offset() == P::MIN { return Err(v); }
        self.store_mut().push_front(v);
        Self::A::on_push_front(self.translator_mut());
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
impl<'a, T, P, const CAP: usize> BlockMutTrait<'a>
for RawBlock<'a, T, P, Prepend, VecStore<T, CAP>>
where
    T: Sized + 'a,
    P: BlockIndex,
{
    type A = Prepend;

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

    ///cold: physical push_front into the reserved low range (the back, for Prepend);
    ///on_push_front bumps inner_offset to cancel the phys shift. refuses at offset==MIN.
    fn try_insert_back(&mut self, v: T) -> Result<P, T> {
        if self.translator().inner_offset() == P::MIN { return Err(v); }
        self.store_mut().push_front(v);
        Self::A::on_push_front(self.translator_mut());
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
