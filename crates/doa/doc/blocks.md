```rust
//!`Block` = store + translator + block data, carrying a `Mode` by type. two
//!surfaces: `BlockTrait` (shared read + basic mut) and `BlockOps` (the per-mode
//!slot surface — a trait, not inherent methods, so the tree-ops layer can call it
//!generically). `Mode` owns the store type and the initial translator params.
//!invariants: `find_slot`/`find_2_slots` re-translate `pos`/`pin` after a grow
//!(addrs stable, pos remap via the returned composed `GrewFixup`); every
//!find/slide applies its fixup to the block's own `BlockData` before returning (a
//!bare `grow_and_spread` does not — its caller applies the fixup); position order
//!(pos 0 = min) is preserved by every op.
///L0017
pub type UniformBlock<'block, N, A, D, O> = Block<'block, N, A, Uniform, D, O>;
///L0018
pub type AnchoredBlock<'block, N, A, D, O> = Block<'block, N, A, Anchored<O>, D, O>;
///L0019
pub type PluripotentBlock<'block, N, A, D, O> = Block<'block, N, A, Pluripotent, D, O>;
///L0024
///no-pin full-range block (no insertion pin; VecStore, `SHIFT = BIT_WIDTH`). used
///by trees that grow by splitting (the root can't stay at a fixed position anyway)
///and other consumers that don't pin.
pub struct Uniform;
///L0028
///root pinned at a fixed addr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX; VecStore); `find_slot`/`slide_none`/`find_2_slots` implicitly pin
///`a2p(root_addr)` — the root never moves. the caller has no choice but to pin.
pub struct Anchored<O: Ordering>(PhantomData<O>);
///L0033
///sparse both-ends-growable block (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`).
///edge inserts (before-first / after-last) grow the store edge and compensate the
///translator — no element ever moves and addrs stay stable in that case.
///`find_slot` order: budgeted scan → spread + rescan → edge grow.
pub struct Pluripotent;
///L0036
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
///L0051
///the grow-fail error.
pub struct InsufficientMaxCapacity();
///L0055
///a `None` slot opened for insert (a position).
#[derive(Clone, Copy)]
pub struct OpenSlot(pub Pos);
///L0060
///`find_slot` result: an optional grow fixup (apply to live positions) + an optional pending
///slide (apply via `slide_none`). `grew` is the composition of every grow this call did.
///`slide == None` ⇒ exhausted (caller must split).
pub struct FoundSlot {
    pub grew:  Option<GrewFixup>,
    pub slide: Option<NoneSlide>,
}
///L0067
///`find_2_slots` result: the (single) grow this call did, if any, + both slides as
///ONE composed fixup (`DoubleSlide`) — apply the slides in either order.
pub struct Found2Slots {
    pub grew:   Option<GrewFixup>,
    pub slides: DoubleSlide,
}
///L0074
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
    fn make_translator() -> Translator<A>;
}
///L0090
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
    where 'block: 'b;
    ///address get: translate addr→pos. panics if the slot is `None`.
    fn aget<'b>(&'b self, addr: Self::A) -> &'b Self::N
    where 'block: 'b;
    ///addr of first occupied slot, None if empty.
    fn first_addr<'b>(&'b self) -> Option<Self::A>
    where 'block: 'b;
    ///addr of last occupied slot, None if empty.
    fn last_addr<'b>(&'b self) -> Option<Self::A>
    where 'block: 'b;
    fn a2p(&self, addr: Self::A) -> Pos;
    fn p2a(&self, pos: Pos) -> Self::A;
    fn adist(&self, a1: Self::A, a2: Self::A) -> usize;
    fn occupied<'b>(&'b self) -> usize
    where 'block: 'b;
    fn len<'b>(&'b self) -> usize
    where 'block: 'b;
    fn cap<'b>(&'b self) -> usize
    where 'block: 'b;
    // ---- mut surface ----
    fn store_mut(&mut self) -> &mut Self::S;
    fn translator_mut(&mut self) -> &mut Translator<Self::A>;
    fn set_data(&mut self, m: Self::BlockData);
    fn data_mut(&mut self) -> &mut Self::BlockData;
    ///place the initialized node into the opened slot. see `Store::insert`.
    fn insert(&mut self, slot: OpenSlot, v: Self::N);
    ///position mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, pos: Pos) -> &'b mut Self::N
    where 'block: 'b;
    ///address mut get. panics if the slot is `None`.
    fn aget_mut<'b>(&'b mut self, addr: Self::A) -> &'b mut Self::N
    where 'block: 'b;
    ///two disjoint `&mut` to occupied positions. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(
        &'b mut self,
        a: Pos,
        b: Pos,
    ) -> (&'b mut Self::N, &'b mut Self::N)
    where
        'block: 'b,
;
    fn free(&mut self, pos: Pos) -> (Self::N, OpenSlot);
    fn swap(&mut self, a: Pos, b: Pos);
    ///swap the record at position `src` with the None at `open`. returns the slot freed at
    ///`src`'s position and the position the record moved to.
    fn swap_open(&mut self, src: Pos, open: OpenSlot) -> (OpenSlot, Pos);
}
///L0201
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
    ) -> Result<Found2Slots, InsufficientMaxCapacity>;
    ///the pinned slot: scans/slides must never move it (Anchored: the root's
    ///slot; free modes: none).
    fn pin_pos(&self) -> Option<Pos>;
    ///scan-only probe: a free slot near `pos` on the `rel` side, `None` ⇒ would
    ///need growth. mutates nothing (no spread/grow/edge-grow) — the space test
    ///for split-vs-grow decisions.
    fn try_find_slot(&self, pos: Pos, rel: Rel) -> Option<NoneSlide>;
    ///`try_find_slot` for two anchors; the slides apply independently.
    fn try_find_2_slots(
        &self,
        pos_a: Pos,
        rel_a: Rel,
        pos_b: Pos,
        rel_b: Rel,
    ) -> Option<DoubleSlide>;
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
    fn cleave_and_spread(&mut self, at: Pos) -> Self;
}
///L0294
impl<'block, A: Addr, N: 'block> Mode<'block, A, N> for Uniform {}
///L0299
impl<'block, O: Ordering, A: Addr, N: 'block> Mode<'block, A, N> for Anchored<O> {}
///L0306
impl<'block, A: Addr, N: 'block> Mode<'block, A, N> for Pluripotent {}
///L0312
impl<'block, N, A, M, D, O> Block<'block, N, A, M, D, O>
where
    N: Sized + 'block,
    A: Addr,
    M: Mode<'block, A, N>,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering {}
///L0357
impl<'block, N, A, M, D, O> BlockTrait<'block> for Block<'block, N, A, M, D, O>
where
    N: Sized + 'block,
    A: Addr,
    M: Mode<'block, A, N>,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering {}
// ---------------------------------------------------------------------------
// BlockOps impls — one per mode, disjoint by `M`.
// ---------------------------------------------------------------------------
///L0400
impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Uniform, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering {}
///L0513
impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Anchored<O>, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering {}
///L0662
impl<'block, N, A, D, O> BlockOps<'block> for Block<'block, N, A, Pluripotent, D, O>
where
    N: Sized + 'block,
    A: Addr,
    D: 'block + Default + Clone + Fixable<A>,
    O: Ordering {}
///L0781
///(shift, inner_offset, outer_offset, init_cap) pinning the root at `O`'s fixed addr.
const fn fr_params<A: Addr, O: Ordering>() -> (u32, A, A, usize);
///L0790
///apply a grow remap to `pos`/`pin` + the block's own data, recording it in `grew`.
fn grew_step<A: Addr, D: Fixable<A>>(
    grew: &mut Option<GrewFixup>,
    g: GrewFixup,
    pos: &mut Pos,
    pin: &mut Option<Pos>,
    data: &mut D,
    tr: &Translator<A>,
);
///L0807
///fixed root addr for an ordering (the `Anchored` pin target).
fn root_addr<O: Ordering, A: Addr>() -> A;
```
