```rust
//!`Block` = store + translator + block data, carrying a `Mode` by type. two
//!surfaces: `BlockTrait` (shared read + basic mut) and `BlockOps` (the per-mode
//!slot surface — a trait, not inherent methods, so the tree-ops layer can call it
//!generically). `Mode` owns the store type and the initial translator params.
//!invariants: `find_slot`/`find_2_slots` re-translate `pos`/`pin` after a grow
//!(vaddrs stable, phys remap via the returned composed `GrewFixup`); every
//!find/slide applies its fixup to the block's own `BlockData` before returning (a
//!bare `grow_and_spread` does not — its caller applies the fixup); physical order
//!(phys 0 = min) is preserved by every op.
///L0018
pub type UniformBlock<'block, N, P, D, O> = Block<'block, N, P, Uniform, D, O>;
///L0019
pub type AnchoredBlock<'block, N, P, D, O> = Block<'block, N, P, Anchored<O>, D, O>;
///L0020
pub type PluripotentBlock<'block, N, P, D, O> = Block<'block, N, P, Pluripotent, D, O>;
///L0025
///no-pin full-range block (no insertion pin; VecStore, `SHIFT = BIT_WIDTH`). used
///by trees that grow by splitting (the root can't stay at a fixed position anyway)
///and other consumers that don't pin.
pub struct Uniform;
///L0029
///root pinned at a fixed vaddr determined by `O` (preorder=0, inorder=MIDPOINT,
///postorder=MAX; VecStore); `find_slot`/`slide_none`/`find_2_slots` implicitly pin
///`v2p(root_vaddr)` — the root never moves. the caller has no choice but to pin.
pub struct Anchored<O: Ordering>(PhantomData<O>);
///L0034
///sparse both-ends-growable block (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`).
///edge inserts (before-first / after-last) grow the store edge and compensate the
///translator — no element ever moves and vaddrs stay stable in that case.
///`find_slot` order: budgeted scan → spread + rescan → edge grow.
pub struct Pluripotent;
///L0037
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
///L0052
///the grow-fail error.
pub struct InsufficientMaxCapacity();
///L0056
///a `None` slot opened for insert (physical).
#[derive(Clone, Copy)]
pub struct OpenSlot(pub usize);
///L0061
///`find_slot` result: an optional grow fixup (apply to live phys) + an optional pending
///slide (apply via `slide_none`). `grew` is the composition of every grow this call did.
///`slide == None` ⇒ exhausted (caller must split).
pub struct FoundSlot {
    pub grew:  Option<GrewFixup>,
    pub slide: Option<NoneSlide>,
}
///L0068
///`find_2_slots` result: the (single) grow this call did, if any, + both slides as
///ONE composed fixup (`TwoSlide`) — apply the slides in either order.
pub struct Found2Slots {
    pub grew:   Option<GrewFixup>,
    pub slides: TwoSlide,
}
///L0075
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
    fn make_translator() -> Translator<P>;
}
///L0091
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
    where 'block: 'b;
    ///virtual get: translate vaddr→phys. panics if the slot is `None`.
    fn vget<'b>(&'b self, ptr: Self::P) -> &'b Self::N
    where 'block: 'b;
    ///vaddr of first occupied slot, None if empty.
    fn first_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b;
    ///vaddr of last occupied slot, None if empty.
    fn last_vaddr<'b>(&'b self) -> Option<Self::P>
    where 'block: 'b;
    fn v2p(&self, virt: Self::P) -> usize;
    fn p2v(&self, phys: usize) -> Self::P;
    fn vdist(&self, v1: Self::P, v2: Self::P) -> usize;
    fn occupied<'b>(&'b self) -> usize
    where 'block: 'b;
    fn len<'b>(&'b self) -> usize
    where 'block: 'b;
    fn cap<'b>(&'b self) -> usize
    where 'block: 'b;
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
;
    ///reserve an opened slot; the caller writes through the returned place.
    /// see `Store::alloc`.
    fn alloc<'b>(&'b mut self, slot: OpenSlot) -> &'b mut MaybeUninit<Self::N>
    where 'block: 'b;
    ///physical mut get. panics if the slot is `None`.
    fn get_mut<'b>(&'b mut self, p: usize) -> &'b mut Self::N
    where 'block: 'b;
    ///virtual mut get. panics if the slot is `None`.
    fn vget_mut<'b>(&'b mut self, ptr: Self::P) -> &'b mut Self::N
    where 'block: 'b;
    ///two disjoint `&mut` to occupied physical slots. panics if `a == b` or either is `None`.
    fn get_disjoint_mut<'b>(
        &'b mut self,
        a: usize,
        b: usize,
    ) -> (&'b mut Self::N, &'b mut Self::N)
    where
        'block: 'b,
;
    fn free(&mut self, p: usize) -> (Self::N, OpenSlot);
    fn swap(&mut self, a: usize, b: usize);
    ///swap the record at phys `src` with the None at `open`. returns the slot freed at
    ///`src`'s phys and the phys the record moved to.
    fn swap_open(&mut self, src: usize, open: OpenSlot) -> (OpenSlot, usize);
}
///L0216
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
    ) -> Result<Found2Slots, InsufficientMaxCapacity>;
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
    fn cleave_and_spread(&mut self, at: usize) -> Self;
}
///L0287
impl<'block, P: BlockIndex, N: 'block> Mode<'block, P, N> for Uniform {}
///L0292
impl<'block, O: Ordering, P: BlockIndex, N: 'block> Mode<'block, P, N> for Anchored<O> {}
///L0299
impl<'block, P: BlockIndex, N: 'block> Mode<'block, P, N> for Pluripotent {}
///L0305
impl<'block, N, P, M, D, O> Block<'block, N, P, M, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    M: Mode<'block, P, N>,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering {}
///L0350
impl<'block, N, P, M, D, O> BlockTrait<'block> for Block<'block, N, P, M, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    M: Mode<'block, P, N>,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering {}
// ---------------------------------------------------------------------------
// BlockOps impls — one per mode, disjoint by `M`.
// ---------------------------------------------------------------------------
///L0393
impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Uniform, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering {}
///L0497
impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Anchored<O>, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering {}
///L0636
impl<'block, N, P, D, O> BlockOps<'block> for Block<'block, N, P, Pluripotent, D, O>
where
    N: Sized + 'block,
    P: BlockIndex,
    D: 'block + Default + Clone + Fixable<P>,
    O: Ordering {}
///L0751
///(shift, inner_offset, outer_offset, init_cap) pinning the root at `O`'s fixed vaddr.
const fn fr_params<P: BlockIndex, O: Ordering>() -> (u32, P, P, usize);
///L0760
///apply a grow remap to `pos`/`pin` + the block's own data, recording it in `grew`.
fn grew_step<P: BlockIndex, D: Fixable<P>>(
    grew: &mut Option<GrewFixup>,
    g: GrewFixup,
    pos: &mut usize,
    pin: &mut Option<usize>,
    data: &mut D,
    tr: &Translator<P>,
);
///L0777
///fixed root vaddr for an ordering (the `Anchored` pin target).
fn root_vaddr<O: Ordering, P: BlockIndex>() -> P;
```
