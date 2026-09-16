//!unbounded slot backends (`VecStore`/`DequeStore`) — cap grows on demand; the
//!addressable limit lives in `Mode::MAX_CAP`, not the store.
//!invariants: `occupied ≤ len ≤ cap`; `push_*`/`grow_*`/`spread` operate on logical
//!slots; `find_slot`/`slide_none` honor a `pin` (kept out of the moved run).
//!slots are `Option<T>`: `Some` = occupied, `None` = hole — values are always
//!initialized; `insert` places one into a hole, the store hands out no
//!write-places. a two-slot open computes both slides before either is applied.
use std::cmp::Ordering::*;
use std::collections::VecDeque;

use crate::{Rel,
            metadata::{DoubleSlide, Fixup, Pos}};

///slide a None `from` -> `to`; caller inserts at `to`. `from==to` => already None.
///delta: shift each moved item's position by. from>to ⇒ None moves left ⇒ items move
///right ⇒ +1. from<to ⇒ items move left ⇒ -1. equal ⇒ 0.
///impls `Fixup` (`fix_pos`: `pos += delta`) — see metadata.rs.
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from:  Pos,
    pub to:    Pos,
    pub delta: isize,
}

///which side the nearest None was found on (slice-relative index).
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}

///forward-only `ExactSizeIterator` over a store's `Some` refs. `len()` is the `Some` count
///(set at construction from `occupied`), so it stays exact despite filtering.
pub(crate) struct SomeIter<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> {
    inner:     I,
    remaining: usize,
}

///Vec-backed store. slots are `Option<T>`: `Some` = occupied, `None` = hole.
pub struct VecStore<T> {
    buf:      Vec<Option<T>>,
    occupied: usize,
}

///VecDeque-backed store. wrap-aware: cross-slice logic for find/slide/spread/split
///at the wrap boundary. slots are `Option<T>` (see `VecStore`).
pub struct DequeStore<T> {
    buf:      VecDeque<Option<T>>,
    occupied: usize,
}

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
    ) -> Option<DoubleSlide> {
        let dist = pos_a.abs_diff(pos_b);
        if dist >= 3 {
            let r = (dist - 1) / 2;
            if let (Some(sa), Some(sb)) = (
                self.find_slot(pos_a, rel_a, r.min(budget), pin),
                self.find_slot(pos_b, rel_b, r.min(budget), pin),
            ) {
                debug_assert!(
                    !slides_interfere(&sa, &sb, pos_a, pos_b),
                    "sphere pass: disjoint by construction"
                );
                return Some(DoubleSlide { a: sa, b: sb });
            }
        }
        let sa = self.find_slot(pos_a, rel_a, budget, pin)?;
        let sb = self.find_slot(pos_b, rel_b, budget, pin)?;
        if slides_interfere(&sa, &sb, pos_a, pos_b) {
            return None;
        }
        Some(DoubleSlide { a: sa, b: sb })
    }

    fn swap(&mut self, a: Pos, b: Pos);

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
    fn with_capacity(n: usize) -> Self {
        let mut s = Self::new();
        let _ = s.grow_back(n);
        s
    }

    ///construct a store from a vec of slots. occupied = count of Some.
    fn from_vec(v: Vec<Option<T>>) -> Self;

    ///deconstruct into a vec of slots.
    fn into_vec(self) -> Vec<Option<T>>;
}

impl NoneSlide {
    pub(crate) fn new(from: Pos, to: Pos) -> Self {
        Self { from, to, delta: (from.0 as isize - to.0 as isize).signum() }
    }
}

impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> Iterator for SomeIter<'b, T, I> {
    type Item = &'b T;

    fn next(&mut self) -> Option<&'b T> {
        for slot in self.inner.by_ref() {
            if let Some(t) = slot {
                self.remaining -= 1;
                return Some(t);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> ExactSizeIterator for SomeIter<'b, T, I> {
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl<'b, T: 'b, I: DoubleEndedIterator<Item = &'b Option<T>>> DoubleEndedIterator
    for SomeIter<'b, T, I>
{
    fn next_back(&mut self) -> Option<&'b T> {
        for slot in self.inner.by_ref().rev() {
            if let Some(t) = slot {
                self.remaining -= 1;
                return Some(t);
            }
        }
        None
    }
}

impl<'a, T: Sized + 'a> Store<'a, T> for VecStore<T> {
    fn new() -> Self {
        Self { buf: Vec::new(), occupied: 0 }
    }

    fn from_vec(v: Vec<Option<T>>) -> Self {
        let occupied = v.iter().filter(|s| s.is_some()).count();
        Self { buf: v, occupied }
    }

    fn into_vec(self) -> Vec<Option<T>> {
        self.buf
    }

    fn get(&self, pos: Pos) -> &T {
        self.buf[pos.0].as_ref().expect("store: None at occupied pos")
    }

    fn get_mut(&mut self, pos: Pos) -> &mut T {
        self.buf[pos.0].as_mut().expect("store: None at occupied pos")
    }

    fn slot(&self, pos: Pos) -> Option<&T> {
        self.buf[pos.0].as_ref()
    }

    fn slot_mut(&mut self, pos: Pos) -> Option<&mut T> {
        self.buf[pos.0].as_mut()
    }

    fn get_disjoint_mut(&mut self, a: Pos, b: Pos) -> (&mut T, &mut T) {
        assert!(a != b, "get_disjoint_mut: a == b");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let (left, right) = self.buf.split_at_mut(hi.0);
        let lo_ref = left[lo.0].as_mut().expect("get_disjoint_mut: None slot");
        let hi_ref = right[0].as_mut().expect("get_disjoint_mut: None slot");
        if a < b { (lo_ref, hi_ref) } else { (hi_ref, lo_ref) }
    }

    fn insert(&mut self, pos: Pos, v: T) {
        let slot = &mut self.buf[pos.0];
        assert!(slot.is_none(), "insert into occupied");
        self.occupied += 1;
        *slot = Some(v);
    }

    fn slide_none(&mut self, ms: NoneSlide, pin: Option<Pos>) -> Pos {
        let (from, to) = (ms.from.0, ms.to.0);
        debug_assert!(pin.is_none_or(|p| p.0 != to), "slide_none: pinned target slot");
        if from == to {
            return Pos(to);
        }
        let (lo, hi) = if from > to { (to, from) } else { (from, to) };
        debug_assert!(
            pin.is_none_or(|p| !(lo < p.0 && p.0 < hi)),
            "slide_none: pin inside run — find_slot must keep slides off the pin"
        );
        if from > to {
            self.buf[lo..=hi].rotate_right(1);
        } else {
            self.buf[lo..=hi].rotate_left(1);
        }
        Pos(to)
    }

    fn find_nearest_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide> {
        let pos = pos.0;
        let pin = pin.map(|p| p.0);
        let buf = self.buf.as_slice();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);

        //clamp to keep the slide off the pin. pin never inside [from,to] after this.
        let (min, max) = match pin {
            Some(p) if p == pos => {
                //pos pinned: search the rel side only. After⇒right, Before⇒left.
                if rel == Rel::After { (pos, max) } else { (min, pos) }
            }
            Some(p) if p < pos => (min.max(p + 1), max), //pin left: left None can't cross it
            Some(p) => (min, max.min(p)),                //pin right: right None can't cross it
            None => (min, max),
        };

        //pos is occupied by contract (the insert anchor); no anchor-None case.
        debug_assert!(buf[pos].is_some());
        //outward scan over [min, pos) down and (pos, max] up.
        let lcnt = pos - min;
        let rcnt = max.saturating_sub(pos + 1);
        let found = match rel {
            Rel::After => {
                dual_scan_outward::<_, true>(buf, buf, pos.wrapping_sub(1), pos + 1, lcnt, rcnt)
            }
            Rel::Before => dual_scan_outward::<_, false>(
                buf,
                buf,
                pos.wrapping_sub(1),
                pos + 1,
                lcnt,
                rcnt,
            ),
        };
        match found {
            NearestNone::Left(l) => Some(NoneSlide::new(
                Pos(l),
                if rel == Rel::Before { Pos(pos - 1) } else { Pos(pos) },
            )),
            NearestNone::Right(r) => Some(NoneSlide::new(
                Pos(r),
                if rel == Rel::After { Pos(pos + 1) } else { Pos(pos) },
            )),
            NearestNone::NotFound => None,
        }
    }

    fn find_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide> {
        let pos = pos.0;
        let pin = pin.map(|p| p.0);
        let buf = self.buf.as_slice();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if rel == Rel::After {
                    (pos, max)
                } else {
                    (min, pos)
                }
            }
            Some(p) if p < pos => (min.max(p + 1), max),
            Some(p) => (min, max.min(p)),
            None => (min, max),
        };
        debug_assert!(buf[pos].is_some());
        let lcnt = pos - min;
        let rcnt = max.saturating_sub(pos + 1);
        if rel == Rel::After {
            if rcnt > 0
                && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
            {
                return Some(NoneSlide::new(Pos(pos + 1 + r), Pos(pos + 1)));
            }
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide::new(Pos(min + l), Pos(pos)));
            }
            None
        } else {
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide::new(Pos(min + l), Pos(pos - 1)));
            }
            if rcnt > 0
                && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
            {
                return Some(NoneSlide::new(Pos(pos + 1 + r), Pos(pos)));
            }
            None
        }
    }

    fn swap(&mut self, a: Pos, b: Pos) {
        self.buf.swap(a.0, b.0)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1);
            self.buf.reserve(target - c);
        }
        self.buf.insert(0, Some(v));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> Pos {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1);
            self.buf.reserve(target - c);
        }
        self.buf.push(Some(v));
        self.occupied += 1;
        Pos(len)
    }

    fn grow_front(&mut self, n: usize) {
        self.buf.splice(0..0, (0..n).map(|_| None));
    }

    fn grow_back(&mut self, n: usize) -> Pos {
        self.buf.extend((0..n).map(|_| None));
        Pos(self.buf.len() - 1)
    }

    fn occupied(&self) -> usize {
        self.occupied
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    fn cap(&self) -> usize {
        self.buf.capacity()
    }

    fn grow(&mut self) {
        let c = self.buf.capacity();
        let target = (c * 2).max(c + 1);
        if target > c {
            self.buf.reserve(target - c);
        }
    }

    fn spread(&mut self, offset: usize) {
        let len = self.buf.len();
        debug_assert!(offset < 2, "spread: offset must be 0 or 1");
        //reserve is relative to len: `len` more slots makes cap ≥ 2*len
        if self.buf.capacity() < len * 2 {
            self.buf.reserve(len);
        }
        self.buf.resize_with(len * 2, || None);

        //take src i -> value to dst=2i+offset, None to the pair gap.
        //reverse: dst (≥ i, == i only at i=0,offset=0's own take) is vacated by an
        //earlier higher-i iter or a fresh tail None — never a live Some.
        for i in (0..len).rev() {
            let v = self.buf[i].take();
            self.buf[2 * i + offset] = v;
        }
    }

    fn free(&mut self, pos: Pos) -> T {
        let slot = &mut self.buf[pos.0];
        assert!(slot.is_some(), "free empty");
        self.occupied -= 1;
        slot.take().expect("free empty")
    }

    fn split(&mut self, at: Pos) -> Self {
        let right_count = self.buf.iter().skip(at.0).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at.0);
        self.occupied -= right_count;
        Self { buf: right, occupied: right_count }
    }

    fn pop_front(&mut self) -> Option<T> {
        if self.buf.is_empty() {
            return None;
        }
        let v = self.buf[0].take();
        if v.is_some() {
            self.occupied -= 1;
        }
        v
    }

    fn pop_back(&mut self) -> Option<T> {
        if self.buf.is_empty() {
            return None;
        }
        let last = self.buf.len() - 1;
        let v = self.buf[last].take();
        if v.is_some() {
            self.occupied -= 1;
        }
        v
    }

    fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }
}

impl<'a, T: Sized + 'a> Store<'a, T> for DequeStore<T> {
    fn new() -> Self {
        Self { buf: VecDeque::new(), occupied: 0 }
    }

    fn from_vec(v: Vec<Option<T>>) -> Self {
        let occupied = v.iter().filter(|s| s.is_some()).count();
        Self { buf: VecDeque::from(v), occupied }
    }

    fn into_vec(self) -> Vec<Option<T>> {
        self.buf.into()
    }

    fn get(&self, pos: Pos) -> &T {
        self.buf[pos.0].as_ref().expect("store: None at occupied pos")
    }

    fn get_mut(&mut self, pos: Pos) -> &mut T {
        self.buf[pos.0].as_mut().expect("store: None at occupied pos")
    }

    fn slot(&self, pos: Pos) -> Option<&T> {
        self.buf[pos.0].as_ref()
    }

    fn slot_mut(&mut self, pos: Pos) -> Option<&mut T> {
        self.buf[pos.0].as_mut()
    }

    fn get_disjoint_mut(&mut self, a: Pos, b: Pos) -> (&mut T, &mut T) {
        assert!(a != b, "get_disjoint_mut: a == b");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        //make the deque's logical range contiguous (indices stable), then split.
        let slice = self.buf.make_contiguous();
        let (left, right) = slice.split_at_mut(hi.0);
        let lo_ref = left[lo.0].as_mut().expect("get_disjoint_mut: None slot");
        let hi_ref = right[0].as_mut().expect("get_disjoint_mut: None slot");
        if a < b { (lo_ref, hi_ref) } else { (hi_ref, lo_ref) }
    }

    fn insert(&mut self, pos: Pos, v: T) {
        let slot = &mut self.buf[pos.0];
        assert!(slot.is_none(), "insert into occupied");
        self.occupied += 1;
        *slot = Some(v);
    }

    fn slide_none(&mut self, ms: NoneSlide, pin: Option<Pos>) -> Pos {
        let (from, to) = (ms.from.0, ms.to.0);
        debug_assert!(pin.is_none_or(|p| p.0 != to), "slide_none: pinned target slot");
        if from == to {
            return Pos(to);
        }
        let (lo, hi) = if from > to { (to, from) } else { (from, to) };
        debug_assert!(
            pin.is_none_or(|p| !(lo < p.0 && p.0 < hi)),
            "slide_none: pin inside run — find_slot must keep slides off the pin"
        );
        let flen = self.buf.as_slices().0.len();

        //run straddles the deque's wrap boundary: per-step swap (order-preserving).
        if lo < flen && hi >= flen {
            let mut hole = from;
            if from > to {
                while hole != to {
                    let next = hole - 1;
                    self.buf.swap(hole, next);
                    hole = next;
                }
            } else {
                while hole != to {
                    let next = hole + 1;
                    self.buf.swap(hole, next);
                    hole = next;
                }
            }
        } else if hi < flen {
            let front = self.buf.as_mut_slices().0;
            if from > to {
                front[lo..=hi].rotate_right(1)
            } else {
                front[lo..=hi].rotate_left(1)
            }
        } else {
            let back = self.buf.as_mut_slices().1;
            let (blo, bhi) = (lo - flen, hi - flen);
            if from > to {
                back[blo..=bhi].rotate_right(1)
            } else {
                back[blo..=bhi].rotate_left(1)
            }
        }
        Pos(to)
    }

    fn find_nearest_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide> {
        let pos = pos.0;
        let pin = pin.map(|p| p.0);
        let (front, back) = self.buf.as_slices();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);

        //clamp to keep the slide off the pin (see VecStore::find_nearest_slot).
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if rel == Rel::After {
                    (pos, max)
                } else {
                    (min, pos)
                }
            }
            Some(p) if p < pos => (min.max(p + 1), max),
            Some(p) => (min, max.min(p)),
            None => (min, max),
        };

        //keypoints - min , boundary, pos,pos+1, max . boundary can lie at any relative position.
        let fl = front.len();
        match pos.cmp(&fl) {
            Less => {
                //pos occupied by contract; outward scan within front, fallback to back.
                debug_assert!(front[pos].is_some());
                let fmax = max.min(fl);
                let scan = |front, back| match rel {
                    Rel::After => dual_scan_outward::<_, true>(
                        front,
                        back,
                        pos.wrapping_sub(1),
                        pos + 1,
                        pos - min,
                        fmax.saturating_sub(pos + 1),
                    ),
                    Rel::Before => dual_scan_outward::<_, false>(
                        front,
                        back,
                        pos.wrapping_sub(1),
                        pos + 1,
                        pos - min,
                        fmax.saturating_sub(pos + 1),
                    ),
                };
                match scan(front, front) {
                    NearestNone::Left(l) => Some(NoneSlide::new(
                        Pos(l),
                        if rel == Rel::Before { Pos(pos - 1) } else { Pos(pos) },
                    )),
                    NearestNone::Right(r) => Some(NoneSlide::new(
                        Pos(r),
                        if rel == Rel::After { Pos(pos + 1) } else { Pos(pos) },
                    )),

                    //front exhausted within budget: any None in back is right of pos.
                    NearestNone::NotFound => back[0..max.saturating_sub(fl)]
                        .iter()
                        .position(|i| i.is_none())
                        .map(|x| {
                            let r = x + fl;
                            NoneSlide::new(
                                Pos(r),
                                if rel == Rel::After { Pos(pos + 1) } else { Pos(pos) },
                            )
                        }),
                }
            }
            Equal => {
                //pos = fl, occupied by contract (buf[fl] = back[0]); left = front
                //[min, fl), right = back (0, max-fl).
                debug_assert!(!back.is_empty() && back[0].is_some());
                let bcnt = max.saturating_sub(fl);
                let scan = |front, back| match rel {
                    Rel::After => dual_scan_outward::<_, true>(
                        front,
                        back,
                        fl.wrapping_sub(1),
                        1,
                        fl - min,
                        bcnt.saturating_sub(1),
                    ),
                    Rel::Before => dual_scan_outward::<_, false>(
                        front,
                        back,
                        fl.wrapping_sub(1),
                        1,
                        fl - min,
                        bcnt.saturating_sub(1),
                    ),
                };
                match scan(front, back) {
                    NearestNone::Left(p) => Some(NoneSlide::new(
                        Pos(p),
                        if rel == Rel::Before { Pos(pos - 1) } else { Pos(pos) },
                    )),
                    NearestNone::Right(p) => {
                        let r = p + fl;
                        Some(NoneSlide::new(
                            Pos(r),
                            if rel == Rel::After { Pos(pos + 1) } else { Pos(pos) },
                        ))
                    }
                    NearestNone::NotFound => None,
                }
            }
            Greater => {
                //pos occupied by contract; outward scan within back, fallback to front.
                let fpos = pos - fl;
                debug_assert!(back[fpos].is_some());
                let fmin = min.saturating_sub(fl);
                let fmax = max.saturating_sub(fl);
                let scan = |front, back| match rel {
                    Rel::After => dual_scan_outward::<_, true>(
                        front,
                        back,
                        fpos.wrapping_sub(1),
                        fpos + 1,
                        fpos - fmin,
                        fmax.saturating_sub(fpos + 1),
                    ),
                    Rel::Before => dual_scan_outward::<_, false>(
                        front,
                        back,
                        fpos.wrapping_sub(1),
                        fpos + 1,
                        fpos - fmin,
                        fmax.saturating_sub(fpos + 1),
                    ),
                };
                match scan(back, back) {
                    NearestNone::Left(l) => {
                        let abs = l + fl;
                        Some(NoneSlide::new(
                            Pos(abs),
                            if rel == Rel::Before { Pos(pos - 1) } else { Pos(pos) },
                        ))
                    }
                    NearestNone::Right(r) => {
                        let abs = r + fl;
                        Some(NoneSlide::new(
                            Pos(abs),
                            if rel == Rel::After { Pos(pos + 1) } else { Pos(pos) },
                        ))
                    }

                    //back exhausted within budget: any None in front is left of pos.
                    NearestNone::NotFound => {
                        front[min.min(fl)..fl].iter().rev().position(|o| o.is_none()).map(|p| {
                            let abs = fl - p - 1;
                            NoneSlide::new(
                                Pos(abs),
                                if rel == Rel::Before { Pos(pos - 1) } else { Pos(pos) },
                            )
                        })
                    }
                }
            }
        }
    }

    fn find_slot(
        &self,
        pos: Pos,
        rel: Rel,
        budget: usize,
        pin: Option<Pos>,
    ) -> Option<NoneSlide> {
        let pos = pos.0;
        let pin = pin.map(|p| p.0);
        let (front, back) = self.buf.as_slices();
        let fl = front.len();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if rel == Rel::After {
                    (pos, max)
                } else {
                    (min, pos)
                }
            }
            Some(p) if p < pos => (min.max(p + 1), max),
            Some(p) => (min, max.min(p)),
            None => (min, max),
        };

        //forward (right) scan: increasing logical index — front[pos+1..fl] then back[..max-fl].
        //returns the absolute index of the first None right of pos within [pos+1, max).
        let scan_right = || -> Option<usize> {
            if pos < fl {
                let fmax = max.min(fl);
                if let Some(r) =
                    front.get(pos + 1..fmax).and_then(|s| s.iter().position(|o| o.is_none()))
                {
                    return Some(pos + 1 + r);
                }
                //front exhausted within budget; continue rightward into back [0, max-fl).
                if max > fl
                    && let Some(r) =
                        back.get(0..max - fl).and_then(|s| s.iter().position(|o| o.is_none()))
                {
                    return Some(fl + r);
                }
                None
            } else if pos == fl {
                //anchor = back[0]; right starts at back[1]
                back.get(1..max.saturating_sub(fl))
                    .and_then(|s| s.iter().position(|o| o.is_none()))
                    .map(|r| fl + 1 + r)
            } else {
                let bp = pos - fl;
                back.get(bp + 1..max.saturating_sub(fl))
                    .and_then(|s| s.iter().position(|o| o.is_none()))
                    .map(|r| fl + bp + 1 + r)
            }
        };

        //backward (left) scan: decreasing logical index — wraps back→front.
        //returns the absolute index of the nearest None left of pos within [min, pos-1].
        let scan_left = || -> Option<usize> {
            if pos == 0 {
                return None;
            }
            if pos <= fl {
                //left starts at front[pos-1] (pos==fl ⇒ front[fl-1]; back[0] is the anchor/right)
                let start = pos - 1;
                front
                    .get(min..=start)
                    .and_then(|s| s.iter().rposition(|o| o.is_none()))
                    .map(|l| min + l)
            } else {
                //pos in back: back[blo..bp] reversed, then front[min..fl] reversed.
                //blo=min-fl keeps the slide off a left pin (pin==pos ⇒ min=pos ⇒ blo=bp ⇒ empty).
                let bp = pos - fl;
                let blo = min.saturating_sub(fl);
                if bp > blo
                    && let Some(l) =
                        back.get(blo..bp).and_then(|s| s.iter().rposition(|o| o.is_none()))
                {
                    //rposition is relative to `blo`, not the back slice's start
                    return Some(fl + blo + l);
                }
                if min < fl {
                    front
                        .get(min..fl)
                        .and_then(|s| s.iter().rposition(|o| o.is_none()))
                        .map(|l| min + l)
                } else {
                    None
                }
            }
        };

        if rel == Rel::After {
            if let Some(r) = scan_right() {
                return Some(NoneSlide::new(Pos(r), Pos(pos + 1)));
            }
            if let Some(l) = scan_left() {
                return Some(NoneSlide::new(Pos(l), Pos(pos)));
            }
            None
        } else {
            if let Some(l) = scan_left() {
                return Some(NoneSlide::new(Pos(l), Pos(pos - 1)));
            }
            if let Some(r) = scan_right() {
                return Some(NoneSlide::new(Pos(r), Pos(pos)));
            }
            None
        }
    }

    fn swap(&mut self, a: Pos, b: Pos) {
        self.buf.swap(a.0, b.0)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_front(Some(v));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> Pos {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_back(Some(v));
        self.occupied += 1;
        Pos(len)
    }

    fn grow_front(&mut self, n: usize) {
        for _ in 0..n {
            self.buf.push_front(None);
        }
    }

    fn grow_back(&mut self, n: usize) -> Pos {
        self.buf.extend((0..n).map(|_| None));
        Pos(self.buf.len() - 1)
    }

    fn occupied(&self) -> usize {
        self.occupied
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    fn cap(&self) -> usize {
        self.buf.capacity()
    }

    fn grow(&mut self) {
        let c = self.buf.capacity();
        let target = (c * 2).max(c + 1);
        if target > c {
            let _ = self.buf.reserve(target - c);
        }
    }

    fn spread(&mut self, offset: usize) {
        let len = self.buf.len();
        debug_assert!(offset < 2, "spread: offset must be 0 or 1");
        //odd len (e.g. len==1, the pow2 base): the mid=len/2 phase split is invalid
        //(it would move the lone element into the upper half). direct i->2i+offset move.
        if len % 2 != 0 {
            self.buf.resize_with(len * 2, || None);
            for i in (0..len).rev() {
                let v = self.buf[i].take();
                self.buf[2 * i + offset] = v;
            }
            return;
        }
        let mid = len / 2;

        // phase1: take upper half [mid,len), push the pair so value lands at 2i+offset
        // and None at 2i+(1-offset) within the new tail [len,2*len). offset 0 -> (v,None);
        // offset 1 -> (None,v). [mid,len) becomes None (space for phase2). ~1.5*len writes.
        for i in mid..len {
            let v = self.buf[i].take();
            if offset == 0 {
                self.buf.push_back(v);
                self.buf.push_back(None);
            } else {
                self.buf.push_back(None);
                self.buf.push_back(v);
            }
        }

        // phase2: spread lower half [0,mid) over [0,len); element j -> 2j+offset. space
        // [mid,len) is None. reverse: 2j+offset>j so slot 2j+offset is vacated (lower) or
        // None (upper); gap 2j+(1-offset) likewise (==j only at j=0,offset=1, our own take).
        // contig -> index the slice (skips deque's per-access (head+i)%cap); wrapped
        // -> make_contiguous's O(n) linearize is a net loss, so eat the deque-index cost.
        if self.buf.as_mut_slices().1.is_empty() {
            let s = self.buf.as_mut_slices().0;
            for j in (0..mid).rev() {
                let v = s[j].take();
                s[2 * j + offset] = v;
            }
        } else {
            for j in (0..mid).rev() {
                let v = self.buf[j].take();
                self.buf[2 * j + offset] = v;
            }
        }
    }

    fn free(&mut self, pos: Pos) -> T {
        let slot = &mut self.buf[pos.0];
        assert!(slot.is_some(), "free empty");
        self.occupied -= 1;
        slot.take().expect("free empty")
    }

    fn split(&mut self, at: Pos) -> Self {
        let right_count = self.buf.iter().skip(at.0).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at.0);
        self.occupied -= right_count;
        Self { buf: right, occupied: right_count }
    }

    fn pop_front(&mut self) -> Option<T> {
        if self.buf.is_empty() {
            return None;
        }
        let v = self.buf[0].take();
        if v.is_some() {
            self.occupied -= 1;
        }
        v
    }

    fn pop_back(&mut self) -> Option<T> {
        if self.buf.is_empty() {
            return None;
        }
        let last = self.buf.len() - 1;
        let v = self.buf[last].take();
        if v.is_some() {
            self.occupied -= 1;
        }
        v
    }

    fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }
}

///the pair can't apply independently: affected spans overlap (a shared slot would
///double-move, or one slide's None-hole lies inside the other's run) or one slide
///moves the other's anchor. spans are closed — conservative.
fn slides_interfere(s1: &NoneSlide, s2: &NoneSlide, a1: Pos, a2: Pos) -> bool {
    let (lo1, hi1) = (s1.from.min(s1.to), s1.from.max(s1.to));
    let (lo2, hi2) = (s2.from.min(s2.to), s2.from.max(s2.to));
    lo1 <= hi2 && lo2 <= hi1 || s1.affects_pos(a2) || s2.affects_pos(a1)
}

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
) -> NearestNone {
    let m = lcnt.min(rcnt);
    for k in 0..m {
        // SAFETY: see function-level invariant; l0-k and r0+k are in-bounds.
        let l_none = unsafe { left.get_unchecked(l0 - k).is_none() };
        let r_none = unsafe { right.get_unchecked(r0 + k).is_none() };
        if l_none & r_none {
            return if D { NearestNone::Right(r0 + k) } else { NearestNone::Left(l0 - k) };
        }
        if l_none {
            return NearestNone::Left(l0 - k);
        }
        if r_none {
            return NearestNone::Right(r0 + k);
        }
    }
    for k in m..lcnt {
        if unsafe { left.get_unchecked(l0 - k).is_none() } {
            return NearestNone::Left(l0 - k);
        }
    }
    for k in m..rcnt {
        if unsafe { right.get_unchecked(r0 + k).is_none() } {
            return NearestNone::Right(r0 + k);
        }
    }
    NearestNone::NotFound
}

//tests unwired for the addr/pos terminology refactor (Pos/Rel signatures) — port
//src/tests/store.rs to the new surface, then re-enable:
//#[cfg(test)]
//#[path = "tests/store.rs"]
//mod tests;
