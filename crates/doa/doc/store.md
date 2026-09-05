```rust
//!unbounded slot backends (`VecStore`/`DequeStore`) — cap grows on demand; the
//!addressable limit lives in `Mode::MAX_CAP`, not the store.
//!invariants: `occupied ≤ len ≤ cap`; `push_*`/`grow_*`/`spread` operate on logical
//!slots; `find_slot`/`slide_none` honor a `pin` (kept out of the moved run).
//!slots are `Option<MaybeUninit<T>>`: the discriminant is the occupancy flag
//!(store-internal, flipped only by `alloc`), the payload exempt from validity until
//!written — the alloc-write-read contract: a slot is read only after its
//!reservation's write completes (the exclusive `&mut MaybeUninit<T>` `alloc`
//!hands out enforces the ordering). both stores impl `Drop` (`assume_init_drop`
//!over `Some` — `MaybeUninit` never drops `T` on its own); dropping a store with a
//!pending reservation is UB — the contract's one sharp edge (subtle_bugs.md §7).
///L0023
///slide a None `from` -> `to`; caller inserts at `to`. `from==to` => already None.
///delta: shift each moved item's phys by. from>to ⇒ None moves left ⇒ items move
///right ⇒ +1. from<to ⇒ items move left ⇒ -1. equal ⇒ 0.
///impls `Fixup` (`fix_p`: `p += delta`) — see metadata.rs.
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from:  usize,
    pub to:    usize,
    pub delta: isize,
}
///L0030
///which side the nearest None was found on (slice-relative index).
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}
///L0038
///forward-only `ExactSizeIterator` over a store's `Some` refs. `len()` is the `Some` count
///(set at construction from `occupied`), so it stays exact despite filtering.
pub(crate) struct SomeIter<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> {
    inner:     I,
    remaining: usize,
}
///L0046
///Vec-backed store. slots are `Option<MaybeUninit<T>>`: the discriminant is the
///occupancy flag (store-internal — flipped by `alloc`), the payload is exempt
///from validity until its reservation's write completes (alloc-write-read).
pub struct VecStore<T> {
    buf:      Vec<Option<MaybeUninit<T>>>,
    occupied: usize,
}
///L0053
///VecDeque-backed store. wrap-aware: cross-slice logic for find/slide/spread/split
///at the wrap boundary. slots are `Option<MaybeUninit<T>>` (see `VecStore`).
pub struct DequeStore<T> {
    buf:      VecDeque<Option<MaybeUninit<T>>>,
    occupied: usize,
}
///L0060
///slot-backend surface: slot access, slide/find/grow/spread/split primitives, and
///the reservation surface.
pub trait Store<'a, T: Sized + 'a>: Sized + 'a {
    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get<'b>(&'b self, ptr: usize) -> &'b T;
    fn get_mut(&mut self, ptr: usize) -> &mut T;
    ///in-bounds slot: `Some` ref if occupied, `None` if empty. used by the block cursor
    ///to scan across gaps without panicking.
    fn slot(&self, p: usize) -> Option<&T>;
    ///in-bounds mut slot: `Some` mut ref if occupied, `None` if empty.
    fn slot_mut(&mut self, p: usize) -> Option<&mut T>;
    ///two disjoint `&mut` to occupied slots `a` and `b`. panics if `a == b` or
    ///either slot is `None` (contract violation). for `split_into` between two
    ///in-block nodes.
    fn get_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut T);
    ///reserve slot `i` (must be None): flip to `Some(uninit)`, occupied += 1, and
    ///hand back the place to write — the caller MUST write through it before the
    ///slot is read (the alloc-write-read contract). the flag never leaves the store.
    fn alloc(&mut self, i: usize) -> &mut MaybeUninit<T>;
    ///(`a` occupied, `b` free) two disjoint muts: the node at `a` plus the
    ///write place at `b`, which is RESERVED here (flip + occupied) — the drain
    ///handoff. the split's drain into `b` is the reservation's write. panics
    ///if `a == b`, `a` is None, or `b` is Some.
    fn alloc_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut MaybeUninit<T>);
    ///slide the None at `from` to `to`; returns `to`. `from==to` => no slide. `pin`, if set, is a
    ///slot whose element must not move.
    ///Precondition: `to != pin` (a pinned `to` can't open). Fastpath rotates (memmove) the run;
    ///the rare pin-in-range, and for the deque a wrap-crossing range, fall back to per-step swaps.
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<usize>) -> usize;
    ///DIR-biased: scan the DIR side first (forward for after, backward for before),
    ///1 read/step sequential, fall to the other side only on exhaustion. `to` is
    ///adjacent on the inserting side (`pos-1`/`pos+1`) when the None is on the DIR
    ///side, else `pos` (pos elem shifts toward the None). `pin`, if set, is a slot
    ///the search must not cross: a slide never spans it. `pos==pin` restricts the
    ///search to the `DIR` side only. pos occupied by contract. Not nearest-None —
    ///may pick a farther None on the opposite side ⇒ larger slide_none.
    fn find_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide>;
    ///nearest None to `pos` within `budget` (bidirectional outward). `to` is
    ///adjacent on the inserting side (`pos-1`/`pos+1`, pos unmoved) when the None
    ///is on that side, else `pos` (pos shifts toward the None). `pin`/`pos==pin` as
    ///find_slot. Minimizes slide distance; slower than find_slot (two-stream scan).
    fn find_nearest_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide>;
    ///two reservations near `pos_a` (side `dir_a`) and `pos_b` (side `dir_b`) whose
    ///slides apply independently in EITHER order — non-overlapping runs, neither
    ///moves the other's anchor. returns `TwoSlide` (one fixup call covers both).
    ///designed for away-pointing sides (each slot opens on its anchor's own side
    ///of the other). two passes: (1) sphere scan — `find_slot` confined to radius
    ///`(|pos_a-pos_b|-1)/2` around each anchor (both its DIR scan and its fallback
    ///are budget-bounded, so nothing escapes the sphere): disjoint spheres ⇒
    ///disjoint runs — independent BY CONSTRUCTION; skipped when the anchors sit
    ///closer than 3 slots (radius 0 finds nothing — including the same-anchor
    ///subtree-first/last case). (2) one requested-side `find_slot` per anchor (a
    ///fallback slide shifts its anchor, preserving the walk-order side — the
    ///wrong-side None is already covered). away-pointing scans that interfere mean
    ///both fell onto the same lone None in budget range ⇒ no pair exists ⇒ None —
    ///the caller spreads and retries. `pin` as `find_slot`.
    fn find_2_slots(
        &self,
        pos_a: usize,
        dir_a: bool,
        pos_b: usize,
        dir_b: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<TwoSlide>;
    fn swap(&mut self, a: usize, b: usize);
    ///increases occupancy.
    fn push_front(&mut self, v: T);
    ///increases occupancy.
    fn push_back(&mut self, v: T) -> usize;
    ///increases len, inserts n Nones, returns max addr.
    fn grow_front(&mut self, n: usize);
    ///increases len, inserts n Nones, returns max addr.
    fn grow_back(&mut self, n: usize) -> usize;
    ///number of Some slots
    fn occupied(&self) -> usize;
    ///number of None + Some slots
    fn len(&self) -> usize;
    ///size of None + Some + MaybeUninit slots
    fn cap(&self) -> usize;
    ///doubles cap
    fn grow(&mut self);
    ///doubles len, moves element at i to 2*i + offset (offset 0 or 1: 0 = evens,
    ///1 = odds). the gap slot is the other of the {2i, 2i+1} pair.
    fn spread(&mut self, offset: usize);
    ///the space at i must be Some or panic. frees it and returns the value.
    fn free(&mut self, i: usize) -> T;
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: usize) -> Self;
    ///take slot 0 if Some (set None), else None. occupancy -1 when Some.
    fn pop_front(&mut self) -> Option<T>;
    ///take slot len-1 if Some (set None), else None. occupancy -1 when Some.
    fn pop_back(&mut self) -> Option<T>;
    fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where 'a: 'b;
    fn new() -> Self;
    ///build a store of `n` Nones — a fresh, empty (occupied=0) buffer of length `n`.
    fn with_capacity(n: usize) -> Self;
    ///construct a store from a vec of slots. occupied = count of Some.
    fn from_vec(v: Vec<Option<T>>) -> Self;
    ///deconstruct into a vec of slots.
    fn into_vec(self) -> Vec<Option<T>>;
}
///L0229
impl NoneSlide {}
///L0235
impl<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> Iterator
    for SomeIter<'b, T, I> {}
///L0256
impl<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> ExactSizeIterator
    for SomeIter<'b, T, I> {}
///L0265
impl<'b, T: 'b, I: DoubleEndedIterator<Item = &'b Option<MaybeUninit<T>>>> DoubleEndedIterator
    for SomeIter<'b, T, I> {}
///L0280
impl<'a, T: Sized + 'a> Store<'a, T> for VecStore<T> {}
///L0611
impl<T> Drop for VecStore<T> {}
///L0625
impl<'a, T: Sized + 'a> Store<'a, T> for DequeStore<T> {}
///L1171
impl<T> Drop for DequeStore<T> {}
///L1187
///`Some` ⇒ written (the alloc-write-read contract: a slot is read only after its
///reservation's write has completed — the exclusive `&mut` handed out by `alloc`
///enforces the ordering in practice). SAFETY: `m` comes from an occupied slot.
#[inline]
unsafe fn assume_ref<'a, T>(m: &'a MaybeUninit<T>) -> &'a T;
///L1192
///mut variant. SAFETY: as `assume_ref`.
#[inline]
unsafe fn assume_mut<'a, T>(m: &'a mut MaybeUninit<T>) -> &'a mut T;
///L1199
///the pair can't apply independently: affected spans overlap (a shared slot would
///double-move, or one slide's None-hole lies inside the other's run) or one slide
///moves the other's anchor. spans are closed — conservative.
fn slides_interfere(s1: &NoneSlide, s2: &NoneSlide, a1: usize, a2: usize) -> bool;
///L1214
///outward nearest-None scan: `left` at `l0, l0-1, …` (lcnt slots, decreasing) and
///`right` at `r0, r0+1, …` (rcnt slots, increasing). D tie-breaks equidistant hits
///(false⇒left, true⇒right). the caller checks the anchor slot separately, so l0/r0
///are the first real candidates and neither equals the anchor.
///
/// SAFETY: every accessed left index is in `[0, left.len())` and every right index
/// in `[0, right.len())`. accessed left = `l0-k` for k in `[0,lcnt)` ⇒ in
/// `[l0-lcnt+1, l0]`; accessed right = `r0+k` for k in `[0,rcnt)` ⇒ in `[r0, r0+rcnt-1]`.
#[inline]
fn dual_scan_outward<T: Sized, const D: bool>(
    left: &[Option<T>],
    right: &[Option<T>],
    l0: usize,
    r0: usize,
    lcnt: usize,
    rcnt: usize,
) -> NearestNone;
///L1252
#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
```
