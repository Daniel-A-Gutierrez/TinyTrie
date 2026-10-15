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
///L0020
///slide a None `from` -> `to`; caller inserts at `to`. `from==to` => already None.
///delta: shift of each moved item. from>to ⇒ items move right ⇒ +1; from<to ⇒ −1; equal ⇒ 0.
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from:  Pos,
    pub to:    Pos,
    pub delta: isize,
}
///L0026
impl NoneSlide {}
///L0039
///which side the nearest None was found on (slice-relative index).
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}
///L0048
///two non-overlapping slides from one `find_2_slots` — apply
///independently in either order. internal plan
#[derive(Clone, Copy, Debug)]
pub struct DoubleSlide {
    pub a: NoneSlide,
    pub b: NoneSlide,
}
///L0056
///one N-None gather: the Nones at `holes` (sorted, all on `rel`'s side
///of `anchor`) compact contiguously beside the anchor. internal plan
#[derive(Clone, Debug)]
pub struct GatherSlide {
    pub anchor: Pos,
    pub rel:    Rel,
    pub holes:  Vec<Pos>,
}
///L0062
impl GatherSlide {}
///L0106
///forward-only `ExactSizeIterator` over a store's `Some` refs. `len()` is the `Some` count
///(set at construction from `occupied`), so it stays exact despite filtering.
pub(crate) struct SomeIter<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> {
    inner:     I,
    remaining: usize,
}
///L0112
///Vec-backed store. slots are `Option<T>`: `Some` = occupied, `None` = hole.
pub struct VecStore<T> {
    buf:      Vec<Option<T>>,
    occupied: usize,
}
///L0119
///VecDeque-backed store. wrap-aware: cross-slice logic for find/slide/spread/split
///at the wrap boundary. slots are `Option<T>` (see `VecStore`).
pub struct DequeStore<T> {
    buf:      VecDeque<Option<T>>,
    occupied: usize,
}
///L0127
///per-element `(from, to)` pairs for one slide, swap-cascade order —
///the hole-adjacent mover first, so a holder applying `swap(from, to)`
///down the list lands each element in a just-vacated slot
fn record_slide<'a, T, S>(s: &S, ms: NoneSlide, out: &mut Vec<(Pos, Pos)>)
where
    T: 'a,
    S: Store<'a, T>,
;
///L0156
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
    ///slide the None at `from` to `to`; returns `to`. `from==to` => no
    ///slide. panics if `to == pin` (a pinned target can't open)
    fn slide_none(&mut self, ms: NoneSlide, pin: Option<Pos>) -> Pos;
    ///fused find+slide: opens a hole for insert near `pos` on `rel`'s
    ///side, recording every moved element as a `(from, to)` pair into
    ///`out` (swap-cascade order). None ⇒ no hole within `budget`
    fn open_slot(
        &mut self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
        out: &mut Vec<(Pos, Pos)>,
    ) -> Option<Pos>;
    ///Rel-biased None search: scan the rel side first, fall to the
    ///other side only on exhaustion. `to` lands adjacent on the
    ///inserting side. NOT nearest-None — may pick a farther None on
    ///the opposite side ⇒ larger slide_none. `pin`: a slot the search
    ///must not cross; `pos==pin` restricts to the rel side; pos
    ///occupied by contract
    fn find_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide>;
    ///nearest None to `pos` within `budget` (bidirectional outward) —
    ///minimizes slide distance; slower than `find_slot` (two-stream
    ///scan). `pin`/`pos==pin` as `find_slot`
    fn find_nearest_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide>;
    ///two opens near `pos_a` (side `rel_a`) and `pos_b` (side `rel_b`)
    ///whose slides apply independently in EITHER order — non-overlapping
    ///runs, neither moves the other's anchor. None ⇒ interference (the
    ///caller spreads and retries); `pin` as `find_slot`
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
    ///nearest `n` Nones on `rel`'s side of `pos` (occupied by contract)
    ///within `budget`, sorted; None if fewer than n. the pin clamp caps
    ///the window, so no chosen hole's gather crosses it
    fn find_n_slots(
        &self,
        pos: Pos,
        rel: Rel,
        n: usize,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<Vec<Pos>>;
    ///apply a gather plan: the chosen Nones compact beside the anchor.
    ///returns the opened slot range. precondition: `pin` not in the
    ///member interval (`find_n_slots`'s clamp)
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
    ///doubles len, moving element at i to 2*i + offset (0 = evens, 1 =
    ///odds); the gap slot is the other of the {2i, 2i+1} pair
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
///L0397
impl NoneSlide {}
///L0403
impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> Iterator for SomeIter<'b, T, I> {}
///L0421
impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> ExactSizeIterator for SomeIter<'b, T, I> {}
///L0428
impl<'b, T: 'b, I: DoubleEndedIterator<Item = &'b Option<T>>> DoubleEndedIterator
    for SomeIter<'b, T, I> {}
///L0442
impl<'a, T: Sized + 'a> Store<'a, T> for VecStore<T> {}
///L0771
impl<'a, T: Sized + 'a> Store<'a, T> for DequeStore<T> {}
///L1335
///the pair can't apply independently: affected spans overlap or one
///slide moves the other's anchor. closed spans — conservative
fn slides_interfere(s1: &NoneSlide, s2: &NoneSlide, a1: Pos, a2: Pos) -> bool;
///L1349
///outward nearest-None scan; D tie-breaks equidistant hits (false⇒left,
///true⇒right). the caller checks the anchor separately — l0/r0 are the
///first real candidates, never the anchor.
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
