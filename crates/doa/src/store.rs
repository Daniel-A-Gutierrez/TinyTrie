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
use std::cmp::Ordering::*;
use std::collections::VecDeque;
use std::mem::MaybeUninit;

use crate::metadata::{Fixup, TwoSlide};

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

///which side the nearest None was found on (slice-relative index).
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}

///forward-only `ExactSizeIterator` over a store's `Some` refs. `len()` is the `Some` count
///(set at construction from `occupied`), so it stays exact despite filtering.
pub(crate) struct SomeIter<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> {
    inner:     I,
    remaining: usize,
}

///Vec-backed store. slots are `Option<MaybeUninit<T>>`: the discriminant is the
///occupancy flag (store-internal — flipped by `alloc`), the payload is exempt
///from validity until its reservation's write completes (alloc-write-read).
pub struct VecStore<T> {
    buf:      Vec<Option<MaybeUninit<T>>>,
    occupied: usize,
}

///VecDeque-backed store. wrap-aware: cross-slice logic for find/slide/spread/split
///at the wrap boundary. slots are `Option<MaybeUninit<T>>` (see `VecStore`).
pub struct DequeStore<T> {
    buf:      VecDeque<Option<MaybeUninit<T>>>,
    occupied: usize,
}

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
    ) -> Option<TwoSlide> {
        let dist = pos_a.abs_diff(pos_b);
        if dist >= 3 {
            let r = (dist - 1) / 2;
            if let (Some(sa), Some(sb)) = (
                self.find_slot(pos_a, dir_a, r.min(budget), pin),
                self.find_slot(pos_b, dir_b, r.min(budget), pin),
            ) {
                debug_assert!(
                    !slides_interfere(&sa, &sb, pos_a, pos_b),
                    "sphere pass: disjoint by construction"
                );
                return Some(TwoSlide { a: sa, b: sb });
            }
        }
        let sa = self.find_slot(pos_a, dir_a, budget, pin)?;
        let sb = self.find_slot(pos_b, dir_b, budget, pin)?;
        if slides_interfere(&sa, &sb, pos_a, pos_b) {
            return None;
        }
        Some(TwoSlide { a: sa, b: sb })
    }

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
    pub(crate) fn new(from: usize, to: usize) -> Self {
        Self { from, to, delta: (from as isize - to as isize).signum() }
    }
}

impl<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> Iterator
    for SomeIter<'b, T, I>
{
    type Item = &'b T;

    fn next(&mut self) -> Option<&'b T> {
        for slot in self.inner.by_ref() {
            if let Some(m) = slot {
                self.remaining -= 1;
                //SAFETY: occupied ⇒ written (alloc-write-read)
                return Some(unsafe { assume_ref(m) });
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'b, T: 'b, I: Iterator<Item = &'b Option<MaybeUninit<T>>>> ExactSizeIterator
    for SomeIter<'b, T, I>
{
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl<'b, T: 'b, I: DoubleEndedIterator<Item = &'b Option<MaybeUninit<T>>>> DoubleEndedIterator
    for SomeIter<'b, T, I>
{
    fn next_back(&mut self) -> Option<&'b T> {
        for slot in self.inner.by_ref().rev() {
            if let Some(m) = slot {
                self.remaining -= 1;
                //SAFETY: occupied ⇒ written (alloc-write-read)
                return Some(unsafe { assume_ref(m) });
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
        Self { buf: v.into_iter().map(|o| o.map(MaybeUninit::new)).collect(), occupied }
    }

    ///take the buf out (Drop would block a plain move) — the payloads leave via
    ///`assume_init_read`, so the leftover buf drops empty.
    fn into_vec(mut self) -> Vec<Option<T>> {
        std::mem::take(&mut self.buf)
            .into_iter()
            .map(|o| o.map(|m| unsafe { m.assume_init_read() }))
            .collect()
    }

    fn get(&self, ptr: usize) -> &T {
        self.buf[ptr]
            .as_ref()
            .map(|m| unsafe { assume_ref(m) })
            .expect("store: None at occupied ptr")
    }

    fn get_mut(&mut self, ptr: usize) -> &mut T {
        self.buf[ptr]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("store: None at occupied ptr")
    }

    fn slot(&self, p: usize) -> Option<&T> {
        self.buf[p].as_ref().map(|m| unsafe { assume_ref(m) })
    }

    fn slot_mut(&mut self, p: usize) -> Option<&mut T> {
        self.buf[p].as_mut().map(|m| unsafe { assume_mut(m) })
    }

    fn get_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut T) {
        assert!(a != b, "get_disjoint_mut: a == b");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let (left, right) = self.buf.split_at_mut(hi);
        let lo_ref = left[lo]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("get_disjoint_mut: None slot");
        let hi_ref = right[0]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("get_disjoint_mut: None slot");
        if a < b { (lo_ref, hi_ref) } else { (hi_ref, lo_ref) }
    }

    fn alloc(&mut self, i: usize) -> &mut MaybeUninit<T> {
        let slot = &mut self.buf[i];
        assert!(slot.is_none(), "alloc into occupied");
        self.occupied += 1;
        slot.insert(MaybeUninit::uninit())
    }

    fn alloc_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut MaybeUninit<T>) {
        assert!(a != b, "alloc_disjoint_mut: a == b");
        debug_assert!(self.buf[a].is_some(), "alloc_disjoint_mut: a is None");
        debug_assert!(self.buf[b].is_none(), "alloc_disjoint_mut: b is Some");
        self.occupied += 1; //reserve b — the split's drain is the write
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let (left, right) = self.buf.split_at_mut(hi);
        if a < b {
            let x = left[lo]
                .as_mut()
                .map(|m| unsafe { assume_mut(m) })
                .expect("alloc_disjoint_mut: None slot");
            let cell = right[0].insert(MaybeUninit::uninit());
            (x, cell)
        } else {
            let cell = left[lo].insert(MaybeUninit::uninit());
            let x = right[0]
                .as_mut()
                .map(|m| unsafe { assume_mut(m) })
                .expect("alloc_disjoint_mut: None slot");
            (x, cell)
        }
    }

    fn slide_none(&mut self, ms: NoneSlide, pin: Option<usize>) -> usize {
        let (from, to) = (ms.from, ms.to);
        debug_assert!(pin != Some(to), "slide_none: pinned target slot");
        if from == to {
            return to;
        }
        let (lo, hi) = if from > to { (to, from) } else { (from, to) };
        debug_assert!(
            pin.is_none_or(|p| !(lo < p && p < hi)),
            "slide_none: pin inside run — find_slot must keep slides off the pin"
        );
        if from > to {
            self.buf[lo..=hi].rotate_right(1);
        } else {
            self.buf[lo..=hi].rotate_left(1);
        }
        to
    }

    fn find_nearest_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide> {
        let buf = self.buf.as_slice();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);

        //clamp to keep the slide off the pin. pin never inside [from,to] after this.
        let (min, max) = match pin {
            Some(p) if p == pos => {
                //pos pinned: search DIR side only. after(dir=true)⇒right, before⇒left.
                if dir { (pos, max) } else { (min, pos) }
            }
            Some(p) if p < pos => (min.max(p + 1), max), //pin left: left None can't cross it
            Some(p) => (min, max.min(p)),                //pin right: right None can't cross it
            None => (min, max),
        };

        //pos is occupied by contract (the insert anchor); no anchor-None case.
        debug_assert!(buf[pos].is_some());
        //outward scan over [min, pos) down and (pos, max) up.
        let lcnt = pos - min;
        let rcnt = max.saturating_sub(pos + 1);
        match if dir {
            dual_scan_outward::<_, true>(buf, buf, pos.wrapping_sub(1), pos + 1, lcnt, rcnt)
        } else {
            dual_scan_outward::<_, false>(buf, buf, pos.wrapping_sub(1), pos + 1, lcnt, rcnt)
        } {
            NearestNone::Left(l) => Some(NoneSlide::new(l, if !dir { pos - 1 } else { pos })),
            NearestNone::Right(r) => Some(NoneSlide::new(r, if dir { pos + 1 } else { pos })),
            NearestNone::NotFound => None,
        }
    }

    fn find_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide> {
        let buf = self.buf.as_slice();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if dir {
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
        if dir {
            if rcnt > 0
                && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
            {
                return Some(NoneSlide::new(pos + 1 + r, pos + 1));
            }
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide::new(min + l, pos));
            }
            None
        } else {
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide::new(min + l, pos - 1));
            }
            if rcnt > 0
                && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
            {
                return Some(NoneSlide::new(pos + 1 + r, pos));
            }
            None
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.buf.swap(a, b)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1);
            self.buf.reserve(target - c);
        }
        self.buf.insert(0, Some(MaybeUninit::new(v)));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> usize {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1);
            self.buf.reserve(target - c);
        }
        self.buf.push(Some(MaybeUninit::new(v)));
        self.occupied += 1;
        len
    }

    fn grow_front(&mut self, n: usize) {
        self.buf.splice(0..0, (0..n).map(|_| None));
    }

    fn grow_back(&mut self, n: usize) -> usize {
        self.buf.extend((0..n).map(|_| None));
        self.buf.len() - 1
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

        // one pass: take src i -> value to dst=2i+offset, None to the pair gap.
        // reverse so dst (>i, or ==i for the i=0,offset=0 self-take) is vacated first.
        let base = self.buf.as_mut_ptr();
        for i in (0..len).rev() {
            let dst = 2 * i + offset;
            let gap = 2 * i + (1 - offset);

            // SAFETY: i in [0,len) init. dst<len is init (None, take'd by an earlier higher-i
            // iter); dst>=len is uninit spare. gap>=len is uninit spare (write None); gap<len
            // is init and already None (vacated earlier, or our own take at i=0,offset=1).
            let v = unsafe { (*base.add(i)).take() };
            unsafe {
                if dst < len {
                    *base.add(dst) = v;
                } else {
                    base.add(dst).write(v);
                }
                if gap >= len {
                    base.add(gap).write(None);
                }
            }
        }
        unsafe {
            self.buf.set_len(len * 2);
        }
    }

    fn free(&mut self, i: usize) -> T {
        let slot = &mut self.buf[i];
        assert!(slot.is_some(), "free empty");
        self.occupied -= 1;
        unsafe { slot.take().expect("free empty").assume_init_read() }
    }

    fn split(&mut self, at: usize) -> Self {
        let right_count = self.buf.iter().skip(at).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at);
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
        v.map(|m| unsafe { m.assume_init_read() })
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
        v.map(|m| unsafe { m.assume_init_read() })
    }

    fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }
}

impl<T> Drop for VecStore<T> {
    fn drop(&mut self) {
        //Some ⇒ written (the alloc-write-read contract) — `MaybeUninit` never
        //drops `T` on its own, so payloads must be dropped here. dropping a
        //store with a pending reservation (Some not yet written) violates that
        //contract and is UB — the one place it turns dangerous (subtle_bugs.md §7).
        for slot in &mut self.buf {
            if let Some(m) = slot {
                unsafe { m.assume_init_drop() };
            }
        }
    }
}

impl<'a, T: Sized + 'a> Store<'a, T> for DequeStore<T> {
    fn new() -> Self {
        Self { buf: VecDeque::new(), occupied: 0 }
    }

    fn from_vec(v: Vec<Option<T>>) -> Self {
        let occupied = v.iter().filter(|s| s.is_some()).count();
        Self { buf: v.into_iter().map(|o| o.map(MaybeUninit::new)).collect(), occupied }
    }

    ///take the buf out (Drop would block a plain move) — the payloads leave via
    ///`assume_init_read`, so the leftover buf drops empty.
    fn into_vec(mut self) -> Vec<Option<T>> {
        std::mem::take(&mut self.buf)
            .into_iter()
            .map(|o| o.map(|m| unsafe { m.assume_init_read() }))
            .collect()
    }

    fn get(&self, ptr: usize) -> &T {
        self.buf[ptr]
            .as_ref()
            .map(|m| unsafe { assume_ref(m) })
            .expect("store: None at occupied ptr")
    }

    fn get_mut(&mut self, ptr: usize) -> &mut T {
        self.buf[ptr]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("store: None at occupied ptr")
    }

    fn slot(&self, p: usize) -> Option<&T> {
        self.buf[p].as_ref().map(|m| unsafe { assume_ref(m) })
    }

    fn slot_mut(&mut self, p: usize) -> Option<&mut T> {
        self.buf[p].as_mut().map(|m| unsafe { assume_mut(m) })
    }

    fn get_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut T) {
        assert!(a != b, "get_disjoint_mut: a == b");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        //make the deque's logical range contiguous (indices stable), then split.
        let slice = self.buf.make_contiguous();
        let (left, right) = slice.split_at_mut(hi);
        let lo_ref = left[lo]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("get_disjoint_mut: None slot");
        let hi_ref = right[0]
            .as_mut()
            .map(|m| unsafe { assume_mut(m) })
            .expect("get_disjoint_mut: None slot");
        if a < b { (lo_ref, hi_ref) } else { (hi_ref, lo_ref) }
    }

    fn alloc(&mut self, i: usize) -> &mut MaybeUninit<T> {
        let slot = &mut self.buf[i];
        assert!(slot.is_none(), "alloc into occupied");
        self.occupied += 1;
        slot.insert(MaybeUninit::uninit())
    }

    fn alloc_disjoint_mut(&mut self, a: usize, b: usize) -> (&mut T, &mut MaybeUninit<T>) {
        assert!(a != b, "alloc_disjoint_mut: a == b");
        debug_assert!(self.buf[a].is_some(), "alloc_disjoint_mut: a is None");
        debug_assert!(self.buf[b].is_none(), "alloc_disjoint_mut: b is Some");
        self.occupied += 1; //reserve b — the split's drain is the write
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let slice = self.buf.make_contiguous();
        let (left, right) = slice.split_at_mut(hi);
        if a < b {
            let x = left[lo]
                .as_mut()
                .map(|m| unsafe { assume_mut(m) })
                .expect("alloc_disjoint_mut: None slot");
            let cell = right[0].insert(MaybeUninit::uninit());
            (x, cell)
        } else {
            let cell = left[lo].insert(MaybeUninit::uninit());
            let x = right[0]
                .as_mut()
                .map(|m| unsafe { assume_mut(m) })
                .expect("alloc_disjoint_mut: None slot");
            (x, cell)
        }
    }

    fn slide_none(&mut self, ms: NoneSlide, pin: Option<usize>) -> usize {
        let (from, to) = (ms.from, ms.to);
        debug_assert!(pin != Some(to), "slide_none: pinned target slot");
        if from == to {
            return to;
        }
        let (lo, hi) = if from > to { (to, from) } else { (from, to) };
        debug_assert!(
            pin.is_none_or(|p| !(lo < p && p < hi)),
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
        to
    }

    fn find_nearest_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide> {
        let (front, back) = self.buf.as_slices();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);

        //clamp to keep the slide off the pin (see VecStore::find_nearest_slot).
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if dir {
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
                let scan = |front, back| match dir {
                    true => dual_scan_outward::<_, true>(
                        front,
                        back,
                        pos.wrapping_sub(1),
                        pos + 1,
                        pos - min,
                        fmax.saturating_sub(pos + 1),
                    ),
                    false => dual_scan_outward::<_, false>(
                        front,
                        back,
                        pos.wrapping_sub(1),
                        pos + 1,
                        pos - min,
                        fmax.saturating_sub(pos + 1),
                    ),
                };
                match scan(front, front) {
                    NearestNone::Left(l) => {
                        Some(NoneSlide::new(l, if !dir { pos - 1 } else { pos }))
                    }
                    NearestNone::Right(r) => {
                        Some(NoneSlide::new(r, if dir { pos + 1 } else { pos }))
                    }

                    //front exhausted within budget: any None in back is right of pos.
                    NearestNone::NotFound => back[0..max.saturating_sub(fl)]
                        .iter()
                        .position(|i| i.is_none())
                        .map(|x| {
                            let r = x + fl;
                            NoneSlide::new(r, if dir { pos + 1 } else { pos })
                        }),
                }
            }
            Equal => {
                //pos = fl, occupied by contract (buf[fl] = back[0]); left = front
                //[min, fl), right = back (0, max-fl).
                debug_assert!(!back.is_empty() && back[0].is_some());
                let bcnt = max.saturating_sub(fl);
                let scan = |front, back| match dir {
                    true => dual_scan_outward::<_, true>(
                        front,
                        back,
                        fl.wrapping_sub(1),
                        1,
                        fl - min,
                        bcnt.saturating_sub(1),
                    ),
                    false => dual_scan_outward::<_, false>(
                        front,
                        back,
                        fl.wrapping_sub(1),
                        1,
                        fl - min,
                        bcnt.saturating_sub(1),
                    ),
                };
                match scan(front, back) {
                    NearestNone::Left(p) => {
                        Some(NoneSlide::new(p, if !dir { pos - 1 } else { pos }))
                    }
                    NearestNone::Right(p) => {
                        let r = p + fl;
                        Some(NoneSlide::new(r, if dir { pos + 1 } else { pos }))
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
                let scan = |front, back| match dir {
                    true => dual_scan_outward::<_, true>(
                        front,
                        back,
                        fpos.wrapping_sub(1),
                        fpos + 1,
                        fpos - fmin,
                        fmax.saturating_sub(fpos + 1),
                    ),
                    false => dual_scan_outward::<_, false>(
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
                        Some(NoneSlide::new(abs, if !dir { pos - 1 } else { pos }))
                    }
                    NearestNone::Right(r) => {
                        let abs = r + fl;
                        Some(NoneSlide::new(abs, if dir { pos + 1 } else { pos }))
                    }

                    //back exhausted within budget: any None in front is left of pos.
                    NearestNone::NotFound => {
                        front[min.min(fl)..fl].iter().rev().position(|o| o.is_none()).map(|p| {
                            let abs = fl - p - 1;
                            NoneSlide::new(abs, if !dir { pos - 1 } else { pos })
                        })
                    }
                }
            }
        }
    }

    fn find_slot(
        &self,
        pos: usize,
        dir: bool,
        budget: usize,
        pin: Option<usize>,
    ) -> Option<NoneSlide> {
        let (front, back) = self.buf.as_slices();
        let fl = front.len();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);
        let (min, max) = match pin {
            Some(p) if p == pos => {
                if dir {
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

        if dir {
            if let Some(r) = scan_right() {
                return Some(NoneSlide::new(r, pos + 1));
            }
            if let Some(l) = scan_left() {
                return Some(NoneSlide::new(l, pos));
            }
            None
        } else {
            if let Some(l) = scan_left() {
                return Some(NoneSlide::new(l, pos - 1));
            }
            if let Some(r) = scan_right() {
                return Some(NoneSlide::new(r, pos));
            }
            None
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.buf.swap(a, b)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_front(Some(MaybeUninit::new(v)));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> usize {
        let len = self.buf.len();
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_back(Some(MaybeUninit::new(v)));
        self.occupied += 1;
        len
    }

    fn grow_front(&mut self, n: usize) {
        for _ in 0..n {
            self.buf.push_front(None);
        }
    }

    fn grow_back(&mut self, n: usize) -> usize {
        self.buf.extend((0..n).map(|_| None));
        self.buf.len() - 1
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

    fn free(&mut self, i: usize) -> T {
        let slot = &mut self.buf[i];
        assert!(slot.is_some(), "free empty");
        self.occupied -= 1;
        unsafe { slot.take().expect("free empty").assume_init_read() }
    }

    fn split(&mut self, at: usize) -> Self {
        let right_count = self.buf.iter().skip(at).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at);
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
        v.map(|m| unsafe { m.assume_init_read() })
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
        v.map(|m| unsafe { m.assume_init_read() })
    }

    fn iter<'b>(
        &'b self,
    ) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }
}

impl<T> Drop for DequeStore<T> {
    fn drop(&mut self) {
        //as `VecStore`'s: Some ⇒ written (alloc-write-read); a pending reservation
        //at drop is UB (subtle_bugs.md §7).
        for slot in &mut self.buf {
            if let Some(m) = slot {
                unsafe { m.assume_init_drop() };
            }
        }
    }
}

///`Some` ⇒ written (the alloc-write-read contract: a slot is read only after its
///reservation's write has completed — the exclusive `&mut` handed out by `alloc`
///enforces the ordering in practice). SAFETY: `m` comes from an occupied slot.
#[inline]
unsafe fn assume_ref<'a, T>(m: &'a MaybeUninit<T>) -> &'a T {
    unsafe { m.assume_init_ref() }
}
///mut variant. SAFETY: as `assume_ref`.
#[inline]
unsafe fn assume_mut<'a, T>(m: &'a mut MaybeUninit<T>) -> &'a mut T {
    unsafe { m.assume_init_mut() }
}

///the pair can't apply independently: affected spans overlap (a shared slot would
///double-move, or one slide's None-hole lies inside the other's run) or one slide
///moves the other's anchor. spans are closed — conservative.
fn slides_interfere(s1: &NoneSlide, s2: &NoneSlide, a1: usize, a2: usize) -> bool {
    let (lo1, hi1) = (s1.from.min(s1.to), s1.from.max(s1.to));
    let (lo2, hi2) = (s2.from.min(s2.to), s2.from.max(s2.to));
    lo1 <= hi2 && lo2 <= hi1 || s1.affects_p(a2) || s2.affects_p(a1)
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

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
