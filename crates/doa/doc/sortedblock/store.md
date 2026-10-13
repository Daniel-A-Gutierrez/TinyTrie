```rust
//!sortedblock's slot backends (`VecStore`/`DequeStore`) — a copy of
//!treeblock's store evolving a different feedback protocol: moving ops record
//!per-element `(from, to)` pos pairs into a caller-provided `Vec` (swap-cascade
//!order — hole-adjacent mover first — so a holder can apply `swap(from, to)`
//!down the list), instead of handing back fixup structs for the caller to
//!interpret. treeblock's store stays untouched.
//!invariants as treeblock's: `occupied ≤ len ≤ cap`; `push_*`/`grow_*`/`spread`
//!operate on logical slots; `find_slot`/`slide_none` honor a `pin` (kept out
//!of the moved run); slots hold initialized values.
//!multi-slot plans (`DoubleSlide`/`GatherSlide`) stay internal — a pairs-based
//!bulk open is future work.
///L0021
///slide a None `from` -> `to`; caller inserts at `to`. `from==to` => already None.
///delta: shift each moved item's position by. from>to ⇒ None moves left ⇒ items move
///right ⇒ +1. from<to ⇒ items move left ⇒ -1. equal ⇒ 0.
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from:  Pos,
    pub to:    Pos,
    pub delta: isize,
}
///L0027
impl NoneSlide {}
///L0040
///which side the nearest None was found on (slice-relative index).
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}
///L0050
///two non-overlapping slides from one `find_2_slots` — both apply independently
///in either order (disjoint runs). internal plan; no fixup impl (pairs are the
///feedback here).
#[derive(Clone, Copy, Debug)]
pub struct DoubleSlide {
    pub a: NoneSlide,
    pub b: NoneSlide,
}
///L0059
///one N-None gather: the `holes.len()` Nones at `holes` (sorted, all on `rel`'s
///side of `anchor`) compact contiguously beside the anchor; every intervening
///Some crosses the None-run exactly once. internal plan; no fixup impl.
#[derive(Clone, Debug)]
pub struct GatherSlide {
    pub anchor: Pos,
    pub rel:    Rel,
    pub holes:  Vec<Pos>,
}
///L0065
impl GatherSlide {}
///L0109
///forward-only `ExactSizeIterator` over a store's `Some` refs. `len()` is the `Some` count
///(set at construction from `occupied`), so it stays exact despite filtering.
pub(crate) struct SomeIter<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> {
    inner:     I,
    remaining: usize,
}
///L0115
///Vec-backed store. slots are `Option<T>`: `Some` = occupied, `None` = hole.
pub struct VecStore<T> {
    buf:      Vec<Option<T>>,
    occupied: usize,
}
///L0122
///VecDeque-backed store. wrap-aware: cross-slice logic for find/slide/spread/split
///at the wrap boundary. slots are `Option<T>` (see `VecStore`).
pub struct DequeStore<T> {
    buf:      VecDeque<Option<T>>,
    occupied: usize,
}
///L0132
///per-element `(from, to)` pairs for one slide, in swap-cascade order — the
///hole-adjacent mover first, so a holder applying `swap(from, to)` down the
///list lands each element in a just-vacated slot. `delta > 0`: items `[lo, hi)`
///shift right (emit `hi-1` down); `delta < 0`: items `(lo, hi]` shift left
///(emit `lo+1` up).
fn record_slide<'a, T, S>(s: &S, ms: NoneSlide, out: &mut Vec<(Pos, Pos)>)
where
    T: 'a,
    S: Store<'a, T>,
;
///L0159
///slot-backend surface: slot access, slide/find/grow/spread/split primitives, and
///insertion.
pub trait Store<'a, T: Sized + 'a>: Sized + 'a {
    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get<'b>(&'b self, pos: Pos) -> &'b T;
    fn get_mut(&mut self, pos: Pos) -> &mut T;
    ///in-bounds slot: `Some` ref if occupied, `None` if empty. used by the block cursor
    ///to scan across gaps without panicking.
    fn slot(&self, pos: Pos) -> Option<&T>;
    ///in-bounds mut slot: `Some` mut ref if occupied, `None` if empty.
    fn slot_mut(&mut self, pos: Pos) -> Option<&mut T>;
    ///two disjoint `&mut` to occupied slots `a` and `b`. panics if `a == b` or
    ///either slot is `None` (contract violation). for `split_into` between two
    ///in-block nodes.
    fn get_disjoint_mut(&mut self, a: Pos, b: Pos) -> (&mut T, &mut T);
    ///insert initialized `v` into hole `pos`. panics if the slot is `Some`.
    fn insert(&mut self, pos: Pos, v: T);
    ///slide the None at `from` to `to`; returns `to`. `from==to` => no slide. `pin`, if set, is a
    ///slot whose element must not move.
    ///Precondition: `to != pin` (a pinned `to` can't open). Fastpath rotates (memmove) the run;
    ///the rare pin-in-range, and for the deque a wrap-crossing range, fall back to per-step swaps.
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<Pos>) -> Pos;
    ///fused find+slide: opens a hole for insert near `pos` on `rel`'s side and
    ///records every moved element as a `(from, to)` pair into `out` —
    ///swap-cascade order (see `record_slide`). `None` ⇒ no hole within
    ///`budget`. the plan-returning finds + raw `slide_none` stay for
    ///multi-slot choreography.
    fn open_slot(
        &mut self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
        out: &mut Vec<(Pos, Pos)>,
    ) -> Option<Pos>;
    ///Rel-biased: scan the rel side first (forward for After, backward for Before),
    ///1 read/step sequential, fall to the other side only on exhaustion. `to` is
    ///adjacent on the inserting side (`pos-1`/`pos+1`) when the None is on the rel
    ///side, else `pos` (pos elem shifts toward the None). `pin`, if set, is a slot
    ///the search must not cross: a slide never spans it. `pos==pin` restricts the
    ///search to the rel side only. pos occupied by contract. Not nearest-None —
    ///may pick a farther None on the opposite side ⇒ larger slide_none.
    fn find_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide>;
    ///nearest None to `pos` within `budget` (bidirectional outward). `to` is
    ///adjacent on the inserting side (`pos-1`/`pos+1`, pos unmoved) when the None
    ///is on that side, else `pos` (pos shifts toward the None). `pin`/`pos==pin` as
    ///find_slot. Minimizes slide distance; slower than find_slot (two-stream scan).
    fn find_nearest_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide>;
    ///two opens near `pos_a` (side `rel_a`) and `pos_b` (side `rel_b`) whose
    ///slides apply independently in EITHER order — non-overlapping runs, neither
    ///moves the other's anchor. returns `DoubleSlide` (one fixup call covers both).
    ///designed for away-pointing sides (each slot opens on its anchor's own side
    ///of the other). two passes: (1) sphere scan — `find_slot` confined to radius
    ///`(|pos_a-pos_b|-1)/2` around each anchor (both its rel scan and its fallback
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
        pos_a: Pos,
        rel_a: Rel,
        pos_b: Pos,
        rel_b: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<DoubleSlide>;
    fn swap(&mut self, a: Pos, b: Pos);
    ///nearest `n` Nones on `rel`'s side of `pos` (occupied by contract) within
    /// `budget`, sorted; `None` if fewer than n. `pin` as `find_slot` — the
    /// clamp caps the window at the pin, so no chosen hole's gather crosses it
    /// (a gathered span moves every intervening Some).
    fn find_n_slots(
        &self,
        pos: Pos,
        rel: Rel,
        n: usize,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<Vec<Pos>>;
    ///apply a gather plan: the chosen Nones compact beside the anchor, every
    /// member Some crossing the run exactly once — one directional pass (all
    /// members move one way; each slot's own iteration precedes any write into
    /// it, and member finals are distinct). returns the opened slot range.
    /// precondition: `pin` not in the member interval (`find_n_slots`'s clamp).
    fn gather_none(&mut self, g: &GatherSlide, pin: Option<Pos>) -> (Pos, Pos);
    ///increases occupancy.
    fn push_front(&mut self, v: T);
    ///increases occupancy. returns the landed position.
    fn push_back(&mut self, v: T) -> Pos;
    ///increases len, inserts n Nones at the front.
    fn grow_front(&mut self, n: usize);
    ///increases len, inserts n Nones, returns the last position.
    fn grow_back(&mut self, n: usize) -> Pos;
    ///number of Some slots
    fn occupied(&self) -> usize;
    ///number of None + Some slots
    fn len(&self) -> usize;
    ///slot capacity: len + spare
    fn cap(&self) -> usize;
    ///doubles cap
    fn grow(&mut self);
    ///doubles len, moves element at i to 2*i + offset (offset 0 or 1: 0 = evens,
    ///1 = odds). the gap slot is the other of the {2i, 2i+1} pair.
    fn spread(&mut self, offset: usize);
    ///the space at `pos` must be Some or panic. frees it and returns the value.
    fn free(&mut self, pos: Pos) -> T;
    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: Pos) -> Self;
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
///L0408
impl NoneSlide {}
///L0414
impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> Iterator for SomeIter<'b, T, I> {}
///L0432
impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> ExactSizeIterator for SomeIter<'b, T, I> {}
///L0439
impl<'b, T: 'b, I: DoubleEndedIterator<Item = &'b Option<T>>> DoubleEndedIterator
    for SomeIter<'b, T, I> {}
///L0453
impl<'a, T: Sized + 'a> Store<'a, T> for VecStore<T> {}
///L0782
impl<'a, T: Sized + 'a> Store<'a, T> for DequeStore<T> {}
///L1347
///the pair can't apply independently: affected spans overlap (a shared slot would
///double-move, or one slide's None-hole lies inside the other's run) or one slide
///moves the other's anchor. spans are closed — conservative.
fn slides_interfere(s1: &NoneSlide, s2: &NoneSlide, a1: Pos, a2: Pos) -> bool;
///L1362
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
```
