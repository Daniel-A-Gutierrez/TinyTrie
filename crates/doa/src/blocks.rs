//!`Block` = store + translator + block data, carrying a `Mode` by type. two
//!surfaces: `BlockTrait` (shared read + basic mut) and `BlockOps` (the per-mode
//!slot surface — a trait, not inherent methods, so the tree-ops layer can call it
//!generically). `Mode` owns the store type and the initial translator params.
//!invariants: `find_slot`/`find_2_slots` re-translate `pos`/`pin` after a grow
//!(vaddrs stable, phys remap via the returned composed `GrewFixup`); every
//!find/slide applies its fixup to the block's own `BlockData` before returning (a
//!bare `grow_and_spread` does not — its caller applies the fixup); physical order
//!(phys 0 = min) is preserved by every op.
use crate::{Ordering, RootPos,
            index::*,
            metadata::{Fixable, Fixup, GrewFixup, TwoSlide},
            store::{DequeStore, NoneSlide, Store, VecStore},
            translator::{AddressTranslator, Translator}};
use std::marker::PhantomData;
use std::mem::MaybeUninit;

pub type UniformBlock<'block, N, P, D, O> = Block<'block, N, P, Uniform, D, O>;
pub type AnchoredBlock<'block, N, P, D, O> = Block<'block, N, P, Anchored<O>, D, O>;
pub type PluripotentBlock<'block, N, P, D, O> = Block<'block, N, P, Pluripotent, D, O>;

///no-pin full-range block (no insertion pin; VecStore, `SHIFT = BIT_WIDTH`). used
///by trees that grow by splitting (the root can't stay at a fixed position anyway)
///and other consumers that don't pin.
pub struct Uniform;
///root pinned at a fixed vaddr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX; VecStore); `find_slot`/`slide_none`/`find_2_slots` implicitly pin
///`v2p(root_vaddr)` — the root never moves. the caller has no choice but to pin.
pub struct Anchored<O: Ordering>(PhantomData<O>);
///sparse both-ends-growable block (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`).
///edge inserts (before-first / after-last) grow the store edge and compensate the
///translator — no element ever moves and vaddrs stay stable in that case.
///`find_slot` order: budgeted scan → spread + rescan → edge grow.
pub struct Pluripotent;

///store + translator + block data, carrying a `Mode` by type.
pub struct Block<'block, N, P, M, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    M: Mode<'block, P, N>,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    store:      M::S,
    translator: Translator<P>,
    block_data: D,
    _phantom:   PhantomData<(&'block N, O)>,
}

///the grow-fail error.
pub struct InsufficientMaxCapacity();

///a `None` slot opened for insert (physical).
#[derive(Clone, Copy)]
pub struct OpenSlot(pub usize);

///`find_slot` result: an optional grow fixup (apply to live phys) + an optional pending
///slide (apply via `slide_none`). `grew` is the composition of every grow this call did.
///`slide == None` ⇒ exhausted (caller must split).
pub struct FoundSlot {
    pub grew:  Option<GrewFixup>,
    pub slide: Option<NoneSlide>,
}

///`find_2_slots` result: the (single) grow this call did, if any, + both slides as
///ONE composed fixup (`TwoSlide`) — apply the slides in either order.
pub struct Found2Slots {
    pub grew:   Option<GrewFixup>,
    pub slides: TwoSlide,
}

///block mode: the store backend + initial translator params, a bet on a workload.
///consts are the *initial* params — vaddrs may wrap; offsets come into play at splits.
pub trait Mode<'block, P: BlockIndex, N: 'block> {
    type S: Store<'block, N>;
    const INNER_OFFSET: P = P::ZERO;
    const OUTER_OFFSET: P = P::ZERO;
    const SHIFT: u32 = 0;
    ///fresh-block store len (Nones). `insert_root` lands at `INIT_CAP / 2`.
    const INIT_CAP: usize = 1;
    ///max store len this mode's translator can address.
    const MAX_CAP: usize = 1 << P::BIT_WIDTH;
    fn make_translator() -> Translator<P> {
        Translator::new(Self::INNER_OFFSET, Self::OUTER_OFFSET, Self::SHIFT, 0)
    }
}

///shared read + basic mut surface over store+translator+block data; the per-mode
///slot surface (sparse mid-insert, splits) is `BlockOps`.
pub trait BlockTrait<'block>: Sized {
    type N: Sized + 'block;
    type P: BlockIndex;
    type S: Store<'block, Self::N> + 'block;
    ///per-block payload (e.g. `Root` + `Height` for tree blocks; `()` otherwise).
    type BlockData: Fixable<Self::P>;
    type O: Ordering;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'block: 'b;
    fn translator(&self) -> &Translator<Self::P>;
    fn data(&self) -> &Self::BlockData;

    ///physical get. panics if the slot is `None` (caller guarantees `p` occupied).
    fn get<'b>(&'b self, p: usize) -> &'b Self::N
    where 'block: 'b {
        self.store().get(p)
    }
    ///virtual get: translate vaddr→phys. panics if the slot is `None`.
    fn vget<'b>(&'b self, ptr: Self::P) -> &'b Self::N
    where 'block: 'b {
        self.store().get(self.translator().v2p(ptr))
    }
    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        (0..s.len()).find(|&p| s.slot(p).is_some()).map(|p| tr.p2v(p))
    }
    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        (0..s.len()).rev().find(|&p| s.slot(p).is_some()).map(|p| tr.p2v(p))
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
    where 'block: 'b {
        self.store().occupied()
    }
    fn len<'b>(&'b self) -> usize
    where 'block: 'b {
        self.store().len()
    }
    fn cap<'b>(&'b self) -> usize
    where 'block: 'b {
        self.store().cap()
    }

    // ---- mut surface ----
    fn store_mut(&mut self) -> &mut Self::S;
    fn translator_mut(&mut self) -> &mut Translator<Self::P>;
    fn set_data(&mut self, m: Self::BlockData);
    fn data_mut(&mut self) -> &mut Self::BlockData;
    ///(`a` occupied, `b` free) — the drain handoff: `b` is reserved (flip
    ///inside). see `Store::alloc_disjoint_mut`.
    fn alloc_disjoint_mut<'b>(
        &'b mut self,
        a: usize,
        b: OpenSlot,
    ) -> (&'b mut Self::N, &'b mut MaybeUninit<Self::N>)
    where
        'block: 'b,
    {
        self.store_mut().alloc_disjoint_mut(a, b.0)
    }
    ///reserve an opened slot; the caller writes through the returned place.
    /// see `Store::alloc`.
    fn alloc<'b>(&'b mut self, slot: OpenSlot) -> &'b mut MaybeUninit<Self::N>
    where 'block: 'b {
        self.store_mut().alloc(slot.0)
    }

    ///physical mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, p: usize) -> &'b mut Self::N
    where 'block: 'b {
        self.store_mut().get_mut(p)
    }
    ///virtual mut get. panics if the slot is `None`.
    fn vget_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::N
    where 'block: 'b {
        let p = self.translator().v2p(ptr);
        self.store_mut().get_mut(p)
    }
    ///two disjoint `&mut` to occupied physical slots. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(
        &'b mut self,
        a: usize,
        b: usize,
    ) -> (&'b mut Self::N, &'b mut Self::N)
    where
        'block: 'b,
    {
        self.store_mut().get_disjoint_mut(a, b)
    }
    fn free(&mut self, p: usize) -> (Self::N, OpenSlot) {
        (self.store_mut().free(p), OpenSlot(p))
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.store_mut().swap(a, b);
    }
    ///swap the record at phys `src` with the None at `open`. returns the slot freed at
    ///`src`'s phys and the phys the record moved to.
    fn swap_open(&mut self, src: usize, open: OpenSlot) -> (OpenSlot, usize) {
        self.store_mut().swap(src, open.0);
        (OpenSlot(src), open.0)
    }
}

///unified per-mode op surface: sparse mid-insert + split. inherent per-mode methods
///can't be called from generic code (the tree-ops layer) — this trait is that surface.
///every tree-capable mode impls it; `find_slot`/`find_2_slots`/`slide_none`/
///`grow_and_spread`/`cleave*` are per-`Mode` (or mode-overridden defaults).
///every find/slide applies its fixup to the block's own `BlockData` before
///returning; a bare `grow_and_spread` does not — its caller applies the fixup.
pub trait BlockOps<'block>: BlockTrait<'block> {
    ///find a free slot or make space near phys `pos` (occupied by contract) on the
    ///`after`(true)/before(false) side. returns the pending grow fixup + slide; `slide ==
    /// None` ⇒ exhausted (caller must split).
    fn find_slot(&mut self, pos: usize, after: bool) -> FoundSlot;
    ///apply a pending slide; returns the opened slot.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot;
    ///spread: double len, halve shift. vaddrs stable (translator remaps). fails when
    ///shift is exhausted or the mode's MAX_CAP would be exceeded.
    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity>;
    ///two disjoint reservations near `pos_a` (side `dir_a`) and `pos_b` (side
    ///`dir_b`) — the slides apply independently in either order, composed as one
    ///`TwoSlide` fixup. default ladder: pair-scan → forced spread + rescan →
    ///genuine exhaustion. one spread max per call. modes with constraints
    ///override (Anchored pins its root; Pluripotent's edge-grow is not tried by
    ///the default).
    fn find_2_slots(
        &mut self,
        pos_a: usize,
        dir_a: bool,
        pos_b: usize,
        dir_b: bool,
    ) -> Result<Found2Slots, InsufficientMaxCapacity> {
        let budget = Self::P::BIT_WIDTH as usize;
        if let Some(slides) =
            self.store().find_2_slots(pos_a, dir_a, pos_b, dir_b, budget, None)
        {
            return Ok(Found2Slots { grew: None, slides });
        }
        let (mut ga, mut gb) = (pos_a, pos_b);
        let g = self.grow_and_spread()?;
        g.fix_p(&mut ga);
        g.fix_p(&mut gb);
        let tr = self.translator().clone();
        self.data_mut().fixup(&g, &tr);
        match self.store().find_2_slots(ga, dir_a, gb, dir_b, self.len(), None) {
            Some(slides) => Ok(Found2Slots { grew: Some(g), slides }),
            None => Err(InsufficientMaxCapacity()),
        }
    }
    ///split [at, len) into a new block (right), self keeps [0, at). right's translator:
    ///inner += at (preserves right-half vaddrs). right's `BlockData` is cloned as-is —
    ///its phys are left-relative; the caller re-points it. caller guarantees no
    ///right→left refs.
    fn cleave(&mut self, at: usize) -> Self;
    ///split [v_start, v_end) vaddrs into a new block (rotation-remap). the new block's
    ///translator bumps rotation by 1 (interspersing free space). caller guarantees the
    ///range's subtree is fully contained and no right→left refs.
    fn cleave_and_rotate(&mut self, v_start: Self::P, v_end: Self::P) -> Self;
    ///`cleave` then spread the right half: right's shift-1 + inner doubled + store
    ///spread — right-half vaddrs stable (`p2v(2i) == old p2v(i)`), fresh Nones
    ///interspersed for insert headroom. the shift-budget-available variant of
    ///`cleave_and_rotate` (which is the shift-exhausted one). left half unchanged.
    fn cleave_and_spread(&mut self, at: usize) -> Self {
        let shift = self.translator().shift();
        assert!(shift > 0, "cleave_and_spread: shift exhausted — use cleave_and_rotate");
        let mut right = self.cleave(at);
        //p2v_r'(2i) = ((2i + inner') << (shift-1)) must equal p2v_r(i) ⟹ inner' = 2·inner_r.
        //rotation != 0 breaks the doubling — that remap is future work (see notes).
        debug_assert!(
            right.translator().rotation() == 0,
            "cleave_and_spread: rotation != 0 needs the shift&rotate remap"
        );
        let inner = right.translator().inner_offset();
        right.translator_mut().set_inner_offset(inner.wrapping_add(inner));
        right.translator_mut().set_shift(shift - 1);
        right.store_mut().spread(0);
        right
    }
}

impl<'block, P: BlockIndex, N: 'block> Mode<'block, P, N> for Uniform {
    type S = VecStore<N>;
    const SHIFT: u32 = P::BIT_WIDTH as u32;
}

impl<'block, O: Ordering, P: BlockIndex, N: 'block> Mode<'block, P, N> for Anchored<O> {
    type S = VecStore<N>;
    const SHIFT: u32 = fr_params::<P, O>().0;
    const INNER_OFFSET: P = fr_params::<P, O>().1;
    const OUTER_OFFSET: P = fr_params::<P, O>().2;
    const INIT_CAP: usize = fr_params::<P, O>().3;
}
impl<'block, P: BlockIndex, N: 'block> Mode<'block, P, N> for Pluripotent {
    type S = DequeStore<N>;
    const SHIFT: u32 = P::BIT_WIDTH as u32 / 2;
    const MAX_CAP: usize = 1 << P::Half::BIT_WIDTH;
}

impl<'block, N, P, M, D, O> Block<'block, N, P, M, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    M: Mode<'block, P, N>,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    ///fresh block: empty store (INIT_CAP Nones) + the mode's initial translator + default data.
    pub fn new() -> Self {
        Self {
            store:      M::S::with_capacity(M::INIT_CAP),
            translator: M::make_translator(),
            block_data: D::default(),
            _phantom:   PhantomData,
        }
    }

    pub fn from_parts(store: M::S, translator: Translator<P>, block_data: D) -> Self {
        Self { store, translator, block_data, _phantom: PhantomData }
    }

    pub fn into_parts(self) -> (M::S, Translator<P>, D) {
        (self.store, self.translator, self.block_data)
    }

    ///first insert into a fresh block. lands the root at `INIT_CAP/2`
    ///(Anchored: the root's pinned phys). returns the root's phys.
    pub fn insert_root(&mut self, v: N) -> usize {
        assert!(self.store().occupied() == 0, "insert_root: block not empty");
        debug_assert!(self.store().len() > M::INIT_CAP / 2, "insert_root: store too short");
        let mid = M::INIT_CAP / 2;
        self.store_mut().alloc(mid).write(v);
        mid
    }

    ///forward iteration over `Some` slots (exact size = occupied).
    pub fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b N> + ExactSizeIterator<Item = &'b N> + 'b
    where 'block: 'b {
        self.store.iter()
    }
}

impl<'block, N, P, M, D, O> BlockTrait<'block> for Block<'block, N, P, M, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    M: Mode<'block, P, N>,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    type N = N;
    type P = P;
    type S = M::S;
    type BlockData = D;
    type O = O;

    fn store<'b>(&'b self) -> &'b M::S
    where 'block: 'b {
        &self.store
    }
    fn translator(&self) -> &Translator<P> {
        &self.translator
    }
    fn data(&self) -> &D {
        &self.block_data
    }

    fn store_mut(&mut self) -> &mut M::S {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<P> {
        &mut self.translator
    }
    fn set_data(&mut self, m: D) {
        self.block_data = m;
    }
    fn data_mut(&mut self) -> &mut D {
        &mut self.block_data
    }
}

// ---------------------------------------------------------------------------
// BlockOps impls — one per mode, disjoint by `M`.
// ---------------------------------------------------------------------------

impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Uniform, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    ///budgeted scan → proactive spread past 3/4 occupancy → forced spread on miss. a
    ///spread intersperses a None between every slot pair, so the scan after one cannot
    ///miss — those paths panic rather than return a lie. a slide-less `FoundSlot` is
    ///then genuine exhaustion (MAX_CAP reached / shift spent) — the caller must split.
    fn find_slot(&mut self, pos: usize, after: bool) -> FoundSlot {
        let mut pos = pos;
        let mut pin = None;
        let mut found = FoundSlot { grew: None, slide: None };
        if self.occupied() * 4 > self.len() * 3 && self.translator().shift() > 0 {
            if let Ok(g) = self.grow_and_spread() {
                grew_step(
                    &mut found.grew,
                    g,
                    &mut pos,
                    &mut pin,
                    &mut self.block_data,
                    &self.translator,
                );
                found.slide = Some(
                    self.store()
                        .find_slot(pos, after, P::BIT_WIDTH as usize, None)
                        .expect("find_slot: nothing in budget after spread"),
                );
                return found;
            }
        }
        if let Some(ns) = self.store().find_slot(pos, after, P::BIT_WIDTH as usize, None) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == <Uniform as Mode<'block, P, N>>::MAX_CAP {
            return found; //genuine exhaustion
        }
        if let Ok(g) = self.grow_and_spread() {
            grew_step(
                &mut found.grew,
                g,
                &mut pos,
                &mut pin,
                &mut self.block_data,
                &self.translator,
            );
            found.slide = Some(
                self.store()
                    .find_slot(pos, after, self.len(), None)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        found //spread impossible (shift spent under MAX_CAP): genuine exhaustion
    }

    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let open = OpenSlot(self.store_mut().slide_none(ms, None));
        //a slide can move the root (no pin here) — the block's own data follows.
        self.block_data.fixup(&ms, &self.translator);
        open
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Uniform as Mode<'block, P, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }

    fn cleave(&mut self, at: usize) -> Self {
        debug_assert!(at <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        //preserve right-half vaddrs: p2v_new(p-at) == p2v_old(p) => io_new = io_old + at
        translator
            .set_inner_offset(self.translator().inner_offset().wrapping_add(P::from_usize(at)));
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: P, v_end: P) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % P::BIT_WIDTH as u32);
        let mut new_store = <Uniform as Mode<'block, P, N>>::S::with_capacity(len);
        let mut i = self.translator().v2p(v_start);
        let end = self.translator().v2p(v_end);
        while i != end {
            let v = self.translator().p2v(i);
            let new_phys = new_trans.v2p(v);
            let elem = self.store_mut().free(i);
            new_store.alloc(new_phys).write(elem);
            i = (i + 1) % len;
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Anchored<O>, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    ///as `Uniform::find_slot` but the search/slide implicitly pin the root — it never
    ///moves. post-spread the root sits on an even slot, so the interspersed Nones are
    ///never on the pin and the same cannot-miss argument holds.
    fn find_slot(&mut self, pos: usize, after: bool) -> FoundSlot {
        let mut pos = pos;
        let mut pin = Some(self.v2p(root_vaddr::<O, P>()));
        let mut found = FoundSlot { grew: None, slide: None };
        if self.occupied() * 4 > self.len() * 3 && self.translator().shift() > 0 {
            if let Ok(g) = self.grow_and_spread() {
                grew_step(
                    &mut found.grew,
                    g,
                    &mut pos,
                    &mut pin,
                    &mut self.block_data,
                    &self.translator,
                );
                found.slide = Some(
                    self.store()
                        .find_slot(pos, after, P::BIT_WIDTH as usize, pin)
                        .expect("find_slot: nothing in budget after spread"),
                );
                return found;
            }
        }
        if let Some(ns) = self.store().find_slot(pos, after, P::BIT_WIDTH as usize, pin) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == <Anchored<O> as Mode<'block, P, N>>::MAX_CAP {
            return found; //genuine exhaustion
        }
        if let Ok(g) = self.grow_and_spread() {
            grew_step(
                &mut found.grew,
                g,
                &mut pos,
                &mut pin,
                &mut self.block_data,
                &self.translator,
            );
            found.slide = Some(
                self.store()
                    .find_slot(pos, after, self.len(), pin)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        found //spread impossible: genuine exhaustion
    }

    ///root is always pinned — override the `pin=None` of the free modes.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let pin = Some(self.v2p(root_vaddr::<O, P>()));
        let open = OpenSlot(self.store_mut().slide_none(ms, pin));
        self.block_data.fixup(&ms, &self.translator);
        open
    }

    ///as the default, but the root is pinned in every scan.
    fn find_2_slots(
        &mut self,
        pos_a: usize,
        dir_a: bool,
        pos_b: usize,
        dir_b: bool,
    ) -> Result<Found2Slots, InsufficientMaxCapacity> {
        let pin = Some(self.v2p(root_vaddr::<O, P>()));
        let budget = P::BIT_WIDTH as usize;
        if let Some(slides) = self.store().find_2_slots(pos_a, dir_a, pos_b, dir_b, budget, pin)
        {
            return Ok(Found2Slots { grew: None, slides });
        }
        let (mut ga, mut gb) = (pos_a, pos_b);
        let g = self.grow_and_spread()?;
        g.fix_p(&mut ga);
        g.fix_p(&mut gb);
        let pin = Some(self.v2p(root_vaddr::<O, P>())); //grew remaps it
        let tr = self.translator().clone();
        self.data_mut().fixup(&g, &tr);
        match self.store().find_2_slots(ga, dir_a, gb, dir_b, self.len(), pin) {
            Some(slides) => Ok(Found2Slots { grew: Some(g), slides }),
            None => Err(InsufficientMaxCapacity()),
        }
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Anchored<O> as Mode<'block, P, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        //postorder (root at MAX): spread onto odds + halve outer so the root's vaddr holds
        let (spread, shrink_outer) = match O::ROOT_POS {
            RootPos::End => (1usize, true),
            _ => (0usize, false),
        };
        if shrink_outer {
            let tr = self.translator_mut();
            tr.set_outer_offset(tr.outer_offset() >> 1);
        }
        self.store_mut().spread(spread);
        Ok(GrewFixup { shl: 1, shift_offset: spread as u8 })
    }

    fn cleave(&mut self, at: usize) -> Self {
        debug_assert!(at <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        translator
            .set_inner_offset(self.translator().inner_offset().wrapping_add(P::from_usize(at)));
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: P, v_end: P) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % P::BIT_WIDTH as u32);
        let mut new_store = <Anchored<O> as Mode<'block, P, N>>::S::with_capacity(len);
        let mut i = self.translator().v2p(v_start);
        let end = self.translator().v2p(v_end);
        while i != end {
            let v = self.translator().p2v(i);
            let new_phys = new_trans.v2p(v);
            let elem = self.store_mut().free(i);
            new_store.alloc(new_phys).write(elem);
            i = (i + 1) % len;
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Pluripotent, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering,
{
    ///budgeted scan → spread → edge grow. the edge grow is the unified-insert core:
    ///before-first grows the store front (nothing moves; `outer -= 1<<shift` keeps every
    ///vaddr on its element), after-last grows the back. a fresh None is always within
    ///the full-budget scan, so post-grow misses panic. a slide-less `FoundSlot` is then
    ///genuine exhaustion — the caller must split.
    fn find_slot(&mut self, pos: usize, after: bool) -> FoundSlot {
        let mut pos = pos;
        let mut pin = None;
        let mut found = FoundSlot { grew: None, slide: None };
        let budget = P::Half::BIT_WIDTH as usize;
        if let Some(ns) = self.store().find_slot(pos, after, budget, None) {
            found.slide = Some(ns);
            return found;
        }
        if let Ok(g) = self.grow_and_spread() {
            grew_step(
                &mut found.grew,
                g,
                &mut pos,
                &mut pin,
                &mut self.block_data,
                &self.translator,
            );
            found.slide = Some(
                self.store()
                    .find_slot(pos, after, self.len(), None)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        //edge grow: a fresh None at the wanted edge. addressable slots under one
        //translator = 2^BIT_WIDTH >> shift; past that the new vaddr range would overlap.
        let addressable = (1usize << P::BIT_WIDTH) >> self.translator().shift();
        if self.len() >= <Pluripotent as Mode<'block, P, N>>::MAX_CAP.min(addressable) {
            return found; //genuine exhaustion
        }
        if after {
            self.store_mut().grow_back(1);
        } else {
            self.store_mut().grow_front(1);
            //outer -= 1<<shift: existing vaddr v maps to (old phys + 1) — vaddr holders
            //stay valid; only phys holders need the fixup (p → p+1).
            let sh = self.translator().shift();
            let outer = self.translator().outer_offset();
            self.translator_mut()
                .set_outer_offset(outer.wrapping_sub(P::from_usize(1usize << sh)));
            grew_step(
                &mut found.grew,
                GrewFixup { shl: 0, shift_offset: 1 },
                &mut pos,
                &mut pin,
                &mut self.block_data,
                &self.translator,
            );
        }
        found.slide = Some(
            self.store()
                .find_slot(pos, after, self.len(), None)
                .expect("find_slot: full scan missed the fresh edge None"),
        );
        found
    }

    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let open = OpenSlot(self.store_mut().slide_none(ms, None));
        //a slide can move the root (no pin here) — the block's own data follows.
        self.block_data.fixup(&ms, &self.translator);
        open
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Pluripotent as Mode<'block, P, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }

    fn cleave(&mut self, at: usize) -> Self {
        debug_assert!(at <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        translator
            .set_inner_offset(self.translator().inner_offset().wrapping_add(P::from_usize(at)));
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: P, v_end: P) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % P::BIT_WIDTH as u32);
        let mut new_store = <Pluripotent as Mode<'block, P, N>>::S::with_capacity(len);
        let mut i = self.translator().v2p(v_start);
        let end = self.translator().v2p(v_end);
        while i != end {
            let v = self.translator().p2v(i);
            let new_phys = new_trans.v2p(v);
            let elem = self.store_mut().free(i);
            new_store.alloc(new_phys).write(elem);
            i = (i + 1) % len;
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

///(shift, inner_offset, outer_offset, init_cap) pinning the root at `O`'s fixed vaddr.
const fn fr_params<P: BlockIndex, O: Ordering>() -> (u32, P, P, usize) {
    match O::ROOT_POS {
        RootPos::Beginning => (P::BIT_WIDTH as u32, P::ZERO, P::ZERO, 1),
        RootPos::Middle => (P::BIT_WIDTH as u32 - 1, P::ZERO, P::ZERO, 2),
        RootPos::End => (P::BIT_WIDTH as u32, P::ZERO, P::MAX, 1),
    }
}

///apply a grow remap to `pos`/`pin` + the block's own data, recording it in `grew`.
fn grew_step<P: BlockIndex, D: Fixable<P>>(
    grew: &mut Option<GrewFixup>,
    g: GrewFixup,
    pos: &mut usize,
    pin: &mut Option<usize>,
    data: &mut D,
    tr: &Translator<P>,
) {
    g.fix_p(pos);
    if let Some(p) = pin.as_mut() {
        g.fix_p(p);
    }
    data.fixup(&g, tr);
    *grew = Some(g);
}

///fixed root vaddr for an ordering (the `Anchored` pin target).
fn root_vaddr<O: Ordering, P: BlockIndex>() -> P {
    match O::ROOT_POS {
        RootPos::Beginning => P::ZERO,
        RootPos::Middle => P::MIDPOINT,
        RootPos::End => P::MAX,
    }
}
