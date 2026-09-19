//!`Block` = store + translator + block data, carrying a `Mode` by type. two
//!surfaces: `BlockTrait` (shared read + basic mut) and `BlockOps` (the per-mode
//!slot surface — a trait, not inherent methods, so the tree-ops layer can call it
//!generically). `Mode` owns the store type and the initial translator params.
//!invariants: `find_slot`/`find_2_slots` re-translate `pos`/`pin` after a grow
//!(addrs stable, pos remap via the returned composed `GrewFixup`); every
//!find/slide applies its fixup to the block's own `BlockData` before returning (a
//!bare `grow_and_spread` does not — its caller applies the fixup); position order
//!(pos 0 = min) is preserved by every op.
use crate::{Ordering, Rel, RootPos,
            index::*,
            metadata::{DoubleSlide, Fixable, Fixup, GatherSlide, GrewFixup, Pos},
            store::{DequeStore, NoneSlide, Store, VecStore},
            translator::{AddressTranslator, Translator}};
use std::marker::PhantomData;

pub type UniformBlock<'block, N, A, D, O> = Block<'block, N, A, Uniform, D, O>;
pub type AnchoredBlock<'block, N, A, D, O> = Block<'block, N, A, Anchored<O>, D, O>;
pub type PluripotentBlock<'block, N, A, D, O> = Block<'block, N, A, Pluripotent, D, O>;

///no-pin full-range block (no insertion pin; VecStore, `SHIFT = BIT_WIDTH`). used
///by trees that grow by splitting (the root can't stay at a fixed position anyway)
///and other consumers that don't pin.
pub struct Uniform;
///root pinned at a fixed addr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX; VecStore); `find_slot`/`slide_none`/`find_2_slots` implicitly pin
///`a2p(root_addr)` — the root never moves. the caller has no choice but to pin.
pub struct Anchored<O: Ordering>(PhantomData<O>);
///sparse both-ends-growable block (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`).
///edge inserts (before-first / after-last) grow the store edge and compensate the
///translator — no element ever moves and addrs stay stable in that case.
///`find_slot` order: budgeted scan → spread + rescan → edge grow.
pub struct Pluripotent;

///store + translator + block data, carrying a `Mode` by type.
pub struct Block<'block, N, A, M, D, O>
where
    N: Sized + 'block,
    A: Addr,
    M: Mode<'block, A, N>,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering,
{
    store:      M::S,
    translator: Translator<A>,
    block_data: D,
    _phantom:   PhantomData<(&'block N, O)>,
}

///the grow-fail error.
pub struct InsufficientMaxCapacity();

///a `None` slot opened for insert (a position).
#[derive(Clone, Copy)]
pub struct OpenSlot(pub Pos);

///`find_slot` result: an optional grow fixup (apply to live positions) + an optional pending
///slide (apply via `slide_none`). `grew` is the composition of every grow this call did.
///`slide == None` ⇒ exhausted (caller must split).
pub struct FoundSlot {
    pub grew:  Option<GrewFixup>,
    pub slide: Option<NoneSlide>,
}

///`find_2_slots` result: the (single) grow this call did, if any, + both slides as
///ONE composed fixup (`DoubleSlide`) — apply the slides in either order.
pub struct Found2Slots {
    pub grew:   Option<GrewFixup>,
    pub slides: DoubleSlide,
}

///`find_n_slots` result: the grow this call did, if any, + the gather plan.
pub struct FoundGather {
    pub grew:   Option<GrewFixup>,
    pub gather: GatherSlide,
}

///block mode: the store backend + initial translator params, a bet on a workload.
///consts are the *initial* params — addrs may wrap; offsets come into play at splits.
pub trait Mode<'block, A: Addr, N: 'block> {
    type S: Store<'block, N>;
    const INNER_OFFSET: A = A::ZERO;
    const OUTER_OFFSET: A = A::ZERO;
    const SHIFT: u32 = 0;
    ///fresh-block store len (Nones). `insert_root` lands at `INIT_CAP / 2`.
    const INIT_CAP: usize = 1;
    ///max store len this mode's translator can address.
    const MAX_CAP: usize = 1 << A::BIT_WIDTH;
    fn make_translator() -> Translator<A> {
        Translator::new(Self::INNER_OFFSET, Self::OUTER_OFFSET, Self::SHIFT, 0)
    }
}

///shared read + basic mut surface over store+translator+block data; the per-mode
///slot surface (sparse mid-insert, splits) is `BlockOps`.
pub trait BlockTrait<'block>: Sized {
    type N: Sized + 'block;
    type A: Addr;
    type S: Store<'block, Self::N> + 'block;
    ///per-block payload (e.g. `Root` + `Height` for tree blocks; `()` otherwise).
    type BlockData: Fixable<Self::A>;
    type O: Ordering;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'block: 'b;
    fn translator(&self) -> &Translator<Self::A>;
    fn data(&self) -> &Self::BlockData;

    ///position get. panics if the slot is `None` (caller guarantees `pos` occupied).
    fn get<'b>(&'b self, pos: Pos) -> &'b Self::N
    where 'block: 'b {
        self.store().get(pos)
    }
    ///address get: translate addr→pos. panics if the slot is `None`.
    fn aget<'b>(&'b self, addr: Self::A) -> &'b Self::N
    where 'block: 'b {
        self.store().get(self.translator().a2p(addr))
    }
    ///addr of first occupied slot, None if empty.
    fn first_addr<'b>(&'b self) -> Option<Self::A>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        (0..s.len()).find(|&p| s.slot(Pos(p)).is_some()).map(|p| tr.p2a(Pos(p)))
    }
    ///addr of last occupied slot, None if empty.
    fn last_addr<'b>(&'b self) -> Option<Self::A>
    where 'block: 'b {
        let s = self.store();
        let tr = self.translator();
        (0..s.len()).rev().find(|&p| s.slot(Pos(p)).is_some()).map(|p| tr.p2a(Pos(p)))
    }
    fn a2p(&self, addr: Self::A) -> Pos {
        self.translator().a2p(addr)
    }
    fn p2a(&self, pos: Pos) -> Self::A {
        self.translator().p2a(pos)
    }
    fn adist(&self, a1: Self::A, a2: Self::A) -> usize {
        self.translator().adist(a1, a2)
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
    fn translator_mut(&mut self) -> &mut Translator<Self::A>;
    fn set_data(&mut self, m: Self::BlockData);
    fn data_mut(&mut self) -> &mut Self::BlockData;
    ///place the initialized node into the opened slot. see `Store::insert`.
    fn insert(&mut self, slot: OpenSlot, v: Self::N) {
        self.store_mut().insert(slot.0, v)
    }

    ///position mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, pos: Pos) -> &'b mut Self::N
    where 'block: 'b {
        self.store_mut().get_mut(pos)
    }
    ///address mut get. panics if the slot is `None`.
    fn aget_mut<'b>(&'b mut self, addr: Self::A) -> &'b mut Self::N
    where 'block: 'b {
        let pos = self.translator().a2p(addr);
        self.store_mut().get_mut(pos)
    }
    ///two disjoint `&mut` to occupied positions. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(
        &'b mut self,
        a: Pos,
        b: Pos,
    ) -> (&'b mut Self::N, &'b mut Self::N)
    where
        'block: 'b,
    {
        self.store_mut().get_disjoint_mut(a, b)
    }
    fn free(&mut self, pos: Pos) -> (Self::N, OpenSlot) {
        (self.store_mut().free(pos), OpenSlot(pos))
    }
    fn swap(&mut self, a: Pos, b: Pos) {
        self.store_mut().swap(a, b);
    }
    ///swap the record at position `src` with the None at `open`. returns the slot freed at
    ///`src`'s position and the position the record moved to.
    fn swap_open(&mut self, src: Pos, open: OpenSlot) -> (OpenSlot, Pos) {
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
    ///find a free slot or make space near pos `pos` (occupied by contract) on the
    ///`rel` side. returns the pending grow fixup + slide; `slide ==
    /// None` ⇒ exhausted (caller must split).
    fn find_slot(&mut self, pos: Pos, rel: Rel) -> FoundSlot;
    ///apply a pending slide; returns the opened slot.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot;
    ///spread: double len, halve shift. addrs stable (translator remaps). fails when
    ///shift is exhausted or the mode's MAX_CAP would be exceeded.
    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity>;
    ///two disjoint opens near `pos_a` (side `rel_a`) and `pos_b` (side `rel_b`)
    ///— the slides apply independently in either order, composed as one
    ///`DoubleSlide` fixup. default ladder: pair-scan → forced spread + rescan →
    ///genuine exhaustion. one spread max per call. modes with constraints
    ///override (Anchored pins its root; Pluripotent's edge-grow is not tried by
    ///the default).
    fn find_2_slots(
        &mut self,
        pos_a: Pos,
        rel_a: Rel,
        pos_b: Pos,
        rel_b: Rel,
    ) -> Result<Found2Slots, InsufficientMaxCapacity> {
        let budget = Self::A::BIT_WIDTH as usize;
        if let Some(slides) =
            self.store().find_2_slots(pos_a, rel_a, pos_b, rel_b, budget, None)
        {
            return Ok(Found2Slots { grew: None, slides });
        }
        let (mut ga, mut gb) = (pos_a, pos_b);
        let g = self.grow_and_spread()?;
        g.fix_pos(&mut ga);
        g.fix_pos(&mut gb);
        let tr = self.translator().clone();
        self.data_mut().grew_fix(g, &tr);
        match self.store().find_2_slots(ga, rel_a, gb, rel_b, self.len(), None) {
            Some(slides) => Ok(Found2Slots { grew: Some(g), slides }),
            None => Err(InsufficientMaxCapacity()),
        }
    }
    ///n slots near `pos` on `rel`'s side — one N-None gather. default ladder:
    ///budgeted scan → full-len scan → spread + rescan → genuine exhaustion
    ///(one spread max; a post-spread miss means fewer than n Nones exist).
    fn find_n_slots(
        &mut self,
        pos: Pos,
        rel: Rel,
        n: usize,
    ) -> Result<FoundGather, InsufficientMaxCapacity> {
        let budget = Self::A::BIT_WIDTH as usize;
        if let Some(holes) = self.store().find_n_slots(pos, rel, n, budget, self.pin_pos()) {
            return Ok(FoundGather { grew: None, gather: GatherSlide { anchor: pos, rel, holes } });
        }
        //full-len scan before growing (as find_slot's ladder rung)
        if let Some(holes) = self.store().find_n_slots(pos, rel, n, self.len(), self.pin_pos()) {
            return Ok(FoundGather { grew: None, gather: GatherSlide { anchor: pos, rel, holes } });
        }
        let mut p = pos;
        let g = self.grow_and_spread()?;
        g.fix_pos(&mut p);
        let tr = self.translator().clone();
        self.data_mut().grew_fix(g, &tr);
        match self.store().find_n_slots(p, rel, n, self.len(), self.pin_pos()) {
            Some(holes) => Ok(FoundGather {
                grew:   Some(g),
                gather: GatherSlide { anchor: p, rel, holes },
            }),
            None => Err(InsufficientMaxCapacity()),
        }
    }
    ///apply a gather plan; returns the opened slot range. as `slide_none`,
    ///applies the fixup to the block's own `BlockData` before returning.
    fn gather_none(&mut self, g: &GatherSlide) -> (Pos, Pos) {
        let pin = self.pin_pos();
        let slots = self.store_mut().gather_none(g, pin);
        let tr = self.translator().clone();
        self.data_mut().gather_fix(g, &tr);
        slots
    }
    ///the pinned slot: scans/slides must never move it (Anchored: the root's
    ///slot; free modes: none).
    fn pin_pos(&self) -> Option<Pos> {
        None
    }
    ///scan-only probe: a free slot near `pos` on the `rel` side, `None` ⇒ would
    ///need growth. mutates nothing (no spread/grow/edge-grow) — the space test
    ///for split-vs-grow decisions.
    fn try_find_slot(&self, pos: Pos, rel: Rel) -> Option<NoneSlide> {
        self.store().find_slot(pos, rel, self.len(), self.pin_pos())
    }
    ///`try_find_slot` for two anchors; the slides apply independently.
    fn try_find_2_slots(
        &self,
        pos_a: Pos,
        rel_a: Rel,
        pos_b: Pos,
        rel_b: Rel,
    ) -> Option<DoubleSlide> {
        self.store()
            .find_2_slots(pos_a, rel_a, pos_b, rel_b, self.len(), self.pin_pos())
    }
    ///split [at, len) into a new block (right), self keeps [0, at). right's translator:
    ///inner += at (preserves right-half addrs). right's `BlockData` is cloned as-is —
    ///its positions are left-relative; the caller re-points it. caller guarantees no
    ///right→left refs.
    fn cleave(&mut self, at: Pos) -> Self;
    ///split [v_start, v_end) addrs into a new block (rotation-remap). the new block's
    ///translator bumps rotation by 1 (interspersing free space). caller guarantees the
    ///range's subtree is fully contained and no right→left refs.
    fn cleave_and_rotate(&mut self, v_start: Self::A, v_end: Self::A) -> Self;
    ///`cleave` then spread the right half: right's shift-1 + inner doubled + store
    ///spread — right-half addrs stable (`p2a(2i) == old p2a(i)`), fresh Nones
    ///interspersed for insert headroom. the shift-budget-available variant of
    ///`cleave_and_rotate` (which is the shift-exhausted one). left half unchanged.
    fn cleave_and_spread(&mut self, at: Pos) -> Self {
        let shift = self.translator().shift();
        assert!(shift > 0, "cleave_and_spread: shift exhausted — use cleave_and_rotate");
        let mut right = self.cleave(at);
        //p2a_r'(2i) = ((2i + inner') << (shift-1)) must equal p2a_r(i) ⟹ inner' = 2·inner_r.
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

impl<'block, A: Addr, N: 'block> Mode<'block, A, N> for Uniform {
    type S = VecStore<N>;
    const SHIFT: u32 = A::BIT_WIDTH as u32;
}

impl<'block, O: Ordering, A: Addr, N: 'block> Mode<'block, A, N> for Anchored<O> {
    type S = VecStore<N>;
    const SHIFT: u32 = fr_params::<A, O>().0;
    const INNER_OFFSET: A = fr_params::<A, O>().1;
    const OUTER_OFFSET: A = fr_params::<A, O>().2;
    const INIT_CAP: usize = fr_params::<A, O>().3;
}
impl<'block, A: Addr, N: 'block> Mode<'block, A, N> for Pluripotent {
    type S = DequeStore<N>;
    const SHIFT: u32 = A::BIT_WIDTH as u32 / 2;
    const MAX_CAP: usize = 1 << A::Half::BIT_WIDTH;
}

impl<'block, N, A, M, D, O> Block<'block, N, A, M, D, O>
where
    N: Sized + 'block,
    A: Addr,
    M: Mode<'block, A, N>,
    D: 'block + Default + Clone + Fixable<A>,
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

    pub fn from_parts(store: M::S, translator: Translator<A>, block_data: D) -> Self {
        Self { store, translator, block_data, _phantom: PhantomData }
    }

    pub fn into_parts(self) -> (M::S, Translator<A>, D) {
        (self.store, self.translator, self.block_data)
    }

    ///first insert into a fresh block. lands the root at `INIT_CAP/2`
    ///(Anchored: the root's pinned position). returns the root position.
    pub fn insert_root(&mut self, v: N) -> Pos {
        assert!(self.store().occupied() == 0, "insert_root: block not empty");
        debug_assert!(self.store().len() > M::INIT_CAP / 2, "insert_root: store too short");
        let mid = M::INIT_CAP / 2;
        self.store_mut().insert(Pos(mid), v);
        Pos(mid)
    }

    ///forward iteration over `Some` slots (exact size = occupied).
    pub fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b N> + ExactSizeIterator<Item = &'b N> + 'b
    where 'block: 'b {
        self.store.iter()
    }
}

impl<'block, N, A, M, D, O> BlockTrait<'block> for Block<'block, N, A, M, D, O>
where
    N: Sized + 'block,
    A: Addr,
    M: Mode<'block, A, N>,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering,
{
    type N = N;
    type A = A;
    type S = M::S;
    type BlockData = D;
    type O = O;

    fn store<'b>(&'b self) -> &'b M::S
    where 'block: 'b {
        &self.store
    }
    fn translator(&self) -> &Translator<A> {
        &self.translator
    }
    fn data(&self) -> &D {
        &self.block_data
    }

    fn store_mut(&mut self) -> &mut M::S {
        &mut self.store
    }
    fn translator_mut(&mut self) -> &mut Translator<A> {
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

impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Uniform, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering,
{
    ///budgeted scan → proactive spread past 3/4 occupancy → forced spread on miss. a
    ///spread intersperses a None between every slot pair, so the scan after one cannot
    ///miss — those paths panic rather than return a lie. a slide-less `FoundSlot` is
    ///then genuine exhaustion (MAX_CAP reached / shift spent) — the caller must split.
    fn find_slot(&mut self, pos: Pos, rel: Rel) -> FoundSlot {
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
                        .find_slot(pos, rel, A::BIT_WIDTH as usize, None)
                        .expect("find_slot: nothing in budget after spread"),
                );
                return found;
            }
        }
        if let Some(ns) = self.store().find_slot(pos, rel, A::BIT_WIDTH as usize, None) {
            found.slide = Some(ns);
            return found;
        }
        //full-len scan before growing: append-heavy growth densifies the store
        //edge past the budget while mid-span holes remain — a fixed budget
        //scales with nothing, and spreading over a hole-rich store runs the
        //translator out of shift.
        if let Some(ns) = self.store().find_slot(pos, rel, self.len(), None) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == <Uniform as Mode<'block, A, N>>::MAX_CAP {
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
                    .find_slot(pos, rel, self.len(), None)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        found //spread impossible (shift spent under MAX_CAP): genuine exhaustion
    }

    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let open = OpenSlot(self.store_mut().slide_none(ms, None));
        //a slide can move the root (no pin here) — the block's own data follows.
        self.block_data.slide_fix(ms, &self.translator);
        open
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Uniform as Mode<'block, A, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }

    fn cleave(&mut self, at: Pos) -> Self {
        debug_assert!(at.0 <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        //preserve right-half addrs: p2a_new(p-at) == p2a_old(p) => io_new = io_old + at
        translator.set_inner_offset(
            self.translator().inner_offset().wrapping_add(A::from_usize(at.0)),
        );
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: A, v_end: A) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % A::BIT_WIDTH as u32);
        let mut new_store = <Uniform as Mode<'block, A, N>>::S::with_capacity(len);
        let mut i = self.translator().a2p(v_start);
        let end = self.translator().a2p(v_end);
        while i != end {
            let v = self.translator().p2a(i);
            let new_pos = new_trans.a2p(v);
            let elem = self.store_mut().free(i);
            new_store.insert(new_pos, elem);
            i = Pos((i.0 + 1) % len);
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Anchored<O>, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering,
{
    ///as `Uniform::find_slot` but the search/slide implicitly pin the root — it never
    ///moves. post-spread the root sits on an even slot, so the interspersed Nones are
    ///never on the pin and the same cannot-miss argument holds.
    fn find_slot(&mut self, pos: Pos, rel: Rel) -> FoundSlot {
        let mut pos = pos;
        let mut pin = self.pin_pos();
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
                        .find_slot(pos, rel, A::BIT_WIDTH as usize, pin)
                        .expect("find_slot: nothing in budget after spread"),
                );
                return found;
            }
        }
        if let Some(ns) = self.store().find_slot(pos, rel, A::BIT_WIDTH as usize, pin) {
            found.slide = Some(ns);
            return found;
        }
        //full-len scan before growing (as Uniform's — the pin still applies)
        if let Some(ns) = self.store().find_slot(pos, rel, self.len(), pin) {
            found.slide = Some(ns);
            return found;
        }
        if self.len() == <Anchored<O> as Mode<'block, A, N>>::MAX_CAP {
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
                    .find_slot(pos, rel, self.len(), pin)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        found //spread impossible: genuine exhaustion
    }

    ///root is always pinned — override the `pin=None` of the free modes.
    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let pin = self.pin_pos();
        let open = OpenSlot(self.store_mut().slide_none(ms, pin));
        self.block_data.slide_fix(ms, &self.translator);
        open
    }

    ///as the default, but the root is pinned in every scan.
    fn find_2_slots(
        &mut self,
        pos_a: Pos,
        rel_a: Rel,
        pos_b: Pos,
        rel_b: Rel,
    ) -> Result<Found2Slots, InsufficientMaxCapacity> {
        let pin = self.pin_pos();
        let budget = A::BIT_WIDTH as usize;
        if let Some(slides) = self.store().find_2_slots(pos_a, rel_a, pos_b, rel_b, budget, pin)
        {
            return Ok(Found2Slots { grew: None, slides });
        }
        let (mut ga, mut gb) = (pos_a, pos_b);
        let g = self.grow_and_spread()?;
        g.fix_pos(&mut ga);
        g.fix_pos(&mut gb);
        let pin = self.pin_pos(); //grew remaps it
        let tr = self.translator().clone();
        self.data_mut().grew_fix(g, &tr);
        match self.store().find_2_slots(ga, rel_a, gb, rel_b, self.len(), pin) {
            Some(slides) => Ok(Found2Slots { grew: Some(g), slides }),
            None => Err(InsufficientMaxCapacity()),
        }
    }

    fn pin_pos(&self) -> Option<Pos> {
        Some(self.a2p(root_addr::<O, A>()))
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Anchored<O> as Mode<'block, A, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        //postorder (root at MAX): spread onto odds + halve outer so the root's addr holds
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

    fn cleave(&mut self, at: Pos) -> Self {
        debug_assert!(at.0 <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        translator.set_inner_offset(
            self.translator().inner_offset().wrapping_add(A::from_usize(at.0)),
        );
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: A, v_end: A) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % A::BIT_WIDTH as u32);
        let mut new_store = <Anchored<O> as Mode<'block, A, N>>::S::with_capacity(len);
        let mut i = self.translator().a2p(v_start);
        let end = self.translator().a2p(v_end);
        while i != end {
            let v = self.translator().p2a(i);
            let new_pos = new_trans.a2p(v);
            let elem = self.store_mut().free(i);
            new_store.insert(new_pos, elem);
            i = Pos((i.0 + 1) % len);
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Pluripotent, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering,
{
    ///budgeted scan → spread → edge grow. the edge grow is the unified-insert core:
    ///before-first grows the store front (nothing moves; `outer -= 1<<shift` keeps every
    ///addr on its element), after-last grows the back. a fresh None is always within
    ///the full-budget scan, so post-grow misses panic. a slide-less `FoundSlot` is then
    ///genuine exhaustion — the caller must split.
    fn find_slot(&mut self, pos: Pos, rel: Rel) -> FoundSlot {
        let mut pos = pos;
        let mut pin = None;
        let mut found = FoundSlot { grew: None, slide: None };
        let budget = A::Half::BIT_WIDTH as usize;
        if let Some(ns) = self.store().find_slot(pos, rel, budget, None) {
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
                    .find_slot(pos, rel, self.len(), None)
                    .expect("find_slot: full scan missed after spread"),
            );
            return found;
        }
        //edge grow: a fresh None at the wanted edge. addressable slots under one
        //translator = 2^BIT_WIDTH >> shift; past that the new addr range would overlap.
        let addressable = (1usize << A::BIT_WIDTH) >> self.translator().shift();
        if self.len() >= <Pluripotent as Mode<'block, A, N>>::MAX_CAP.min(addressable) {
            return found; //genuine exhaustion
        }
        match rel {
            Rel::After => {
                self.store_mut().grow_back(1);
            }
            Rel::Before => {
                self.store_mut().grow_front(1);
                //outer -= 1<<shift: existing addr v maps to (old pos + 1) — addr holders
                //stay valid; only pos holders need the fixup (pos → pos+1).
                let sh = self.translator().shift();
                let outer = self.translator().outer_offset();
                self.translator_mut()
                    .set_outer_offset(outer.wrapping_sub(A::from_usize(1usize << sh)));
                grew_step(
                    &mut found.grew,
                    GrewFixup { shl: 0, shift_offset: 1 },
                    &mut pos,
                    &mut pin,
                    &mut self.block_data,
                    &self.translator,
                );
            }
        }
        found.slide = Some(
            self.store()
                .find_slot(pos, rel, self.len(), None)
                .expect("find_slot: full scan missed the fresh edge None"),
        );
        found
    }

    fn slide_none(&mut self, ms: NoneSlide) -> OpenSlot {
        let open = OpenSlot(self.store_mut().slide_none(ms, None));
        //a slide can move the root (no pin here) — the block's own data follows.
        self.block_data.slide_fix(ms, &self.translator);
        open
    }

    fn grow_and_spread(&mut self) -> Result<GrewFixup, InsufficientMaxCapacity> {
        let shift = self.translator().shift();
        if shift == 0 || self.store().len() * 2 > <Pluripotent as Mode<'block, A, N>>::MAX_CAP {
            return Err(InsufficientMaxCapacity());
        }
        self.translator_mut().set_shift(shift - 1);
        self.store_mut().spread(0);
        Ok(GrewFixup { shl: 1, shift_offset: 0 })
    }

    fn cleave(&mut self, at: Pos) -> Self {
        debug_assert!(at.0 <= self.store().len(), "cleave: at out of range");
        let right = self.store_mut().split(at);
        let mut translator = self.translator.clone();
        translator.set_inner_offset(
            self.translator().inner_offset().wrapping_add(A::from_usize(at.0)),
        );
        Self::from_parts(right, translator, self.block_data.clone())
    }

    fn cleave_and_rotate(&mut self, v_start: A, v_end: A) -> Self {
        let len = self.store().len();
        let mut new_trans = self.translator.clone();
        new_trans.set_rotation((self.translator().rotation() + 1) % A::BIT_WIDTH as u32);
        let mut new_store = <Pluripotent as Mode<'block, A, N>>::S::with_capacity(len);
        let mut i = self.translator().a2p(v_start);
        let end = self.translator().a2p(v_end);
        while i != end {
            let v = self.translator().p2a(i);
            let new_pos = new_trans.a2p(v);
            let elem = self.store_mut().free(i);
            new_store.insert(new_pos, elem);
            i = Pos((i.0 + 1) % len);
        }
        Self::from_parts(new_store, new_trans, self.block_data.clone())
    }
}

///(shift, inner_offset, outer_offset, init_cap) pinning the root at `O`'s fixed addr.
const fn fr_params<A: Addr, O: Ordering>() -> (u32, A, A, usize) {
    match O::ROOT_POS {
        RootPos::Beginning => (A::BIT_WIDTH as u32, A::ZERO, A::ZERO, 1),
        RootPos::Middle => (A::BIT_WIDTH as u32 - 1, A::ZERO, A::ZERO, 2),
        RootPos::End => (A::BIT_WIDTH as u32, A::ZERO, A::MAX, 1),
    }
}

///apply a grow remap to `pos`/`pin` + the block's own data, recording it in `grew`.
fn grew_step<A: Addr, D: Fixable<A>>(
    grew: &mut Option<GrewFixup>,
    g: GrewFixup,
    pos: &mut Pos,
    pin: &mut Option<Pos>,
    data: &mut D,
    tr: &Translator<A>,
) {
    g.fix_pos(pos);
    if let Some(p) = pin.as_mut() {
        g.fix_pos(p);
    }
    data.grew_fix(g, tr);
    *grew = Some(g);
}

///fixed root addr for an ordering (the `Anchored` pin target).
fn root_addr<O: Ordering, A: Addr>() -> A {
    match O::ROOT_POS {
        RootPos::Beginning => A::ZERO,
        RootPos::Middle => A::MIDPOINT,
        RootPos::End => A::MAX,
    }
}
