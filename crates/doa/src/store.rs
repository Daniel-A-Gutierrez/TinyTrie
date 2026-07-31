use std::cmp::Ordering::*;
use std::{collections::VecDeque,
          ops::Index};

///realistically this is a wrapper over vec<Option<T>> and vecdeque<Option<T>> that limits max cap and provides
///access/shift semantics
///address translation
///dumb insertion
pub(crate) trait Store<'a, T: Sized + 'a>: Sized + 'a {

    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get<'b>(&'b self, ptr: usize) -> &'b T;

    fn get_mut(&mut self, ptr: usize) -> &mut T;

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

    fn swap(&mut self, a: usize, b: usize);

    ///increases occupancy, may not increase cap beyond max
    fn push_front(&mut self, v: T);

    ///increases occupancy, may not increase cap beyond max
    fn push_back(&mut self, v: T) -> usize;

    ///increases len, may not increase cap beyond max.
    ///inserts n Nones up to cap max, returns max addr
    fn grow_front(&mut self, n: usize);

    ///increases len, may not increase cap beyond max.
    ///inserts n Nones up to cap max, returns max addr.
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

    ///The space at i must be None or panic.
    fn insert(&mut self, v: T, i: usize);

    ///the space at i must be Some or panic. returns the removed element.
    fn remove(&mut self, i: usize) -> T;

    ///split buf at `at`: [at, len) move into a new store, drained from self; self keeps [0, at).
    fn split(&mut self, at: usize) -> Self;

    ///split at `at` and odds-gap both halves: self keeps left half at odd slots 2p+1 (even None),
    ///new same-cap store gets right half (reindexed from 0) at odd slots 2k+1. old right-half slots overwritten.
    fn split_and_rotate(&mut self, at: usize) -> Self;

    ///take slot 0 if Some (set None), else None. occupancy -1 when Some.
    fn pop_front(&mut self) -> Option<T>;

    ///take slot len-1 if Some (set None), else None. occupancy -1 when Some.
    fn pop_back(&mut self) -> Option<T>;

    fn iter<'b>(&'b self) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where 'a: 'b;

    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where 'a: 'b;

    fn slots<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where 'a: 'b;

    fn slice_iter<'b>(
        &'b self,
        from: usize,
        to: usize,
    ) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        'a: 'b;

    fn max_capacity() -> usize; //the maximum capacity of the store type.

    fn new() -> Self;
}

///slide a None `from` -> `to`; caller inserts at `to`. `from==to` => already None.
pub struct NoneSlide {
    pub(crate) from: usize,
    pub(crate) to:   usize,
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

impl<'b, T: 'b, I: Iterator<Item = &'b Option<T>>> Iterator for SomeIter<'b, T, I> {
    type Item = &'b T;

    fn next(&mut self) -> Option<&'b T> {
        for slot in self.inner.by_ref() {
            if let Some(v) = slot {
                self.remaining -= 1;
                return Some(v);
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
            if let Some(v) = slot {
                self.remaining -= 1;
                return Some(v);
            }
        }
        None
    }
}

///positioned reader over a store's `Some` slots — distinct from `iter()` (a forward
///`ExactSizeIterator`). `seek` is O(1) (direct index); `next`/`prev`/first-positioning scan
///across `None` gaps. `pos == None` means at-end (no current element).
pub trait Cursor<'b, T: 'b> {

    ///physical slot of the current element, or `None` if at-end.
    fn position(&self) -> Option<usize>;

    ///current element, or `None` if at-end.
    fn current(&self) -> Option<&'b T>;

    ///O(1) jump to physical slot `p`; panics if out of bounds or `None`.
    fn seek(&mut self, p: usize);

    ///advance to the next `Some` slot. returns false iff at-end (no advance).
    fn next(&mut self) -> bool;

    ///advance to the previous `Some` slot. returns false iff already at the first.
    fn prev(&mut self) -> bool;

    ///seek to the nearest `Some` to the beginning (the first occupied slot). its position, or `None` if empty.
    fn first(&mut self) -> Option<usize>;

    ///seek to the nearest `Some` to the end (the last occupied slot). its position, or `None` if empty.
    fn last(&mut self) -> Option<usize>;
}

///cursor backed by direct indexing into `S` (a `Vec`/`VecDeque` of `Option<T>`).
pub(crate) struct SlotCursor<'b, T: 'b, S: Index<usize, Output = Option<T>>> {
    buf:    &'b S,
    nslots: usize,
    pos:    Option<usize>,
}

impl<'b, T: 'b, S: Index<usize, Output = Option<T>>> SlotCursor<'b, T, S> {

    fn new(buf: &'b S, nslots: usize) -> Self {
        let mut c = Self { buf, nslots, pos: None };
        let _ = c.first();
        c
    }

    #[inline]
    fn slot(&self, p: usize) -> &'b T {
        self.buf[p].as_ref().expect("cursor: None at occupied slot")
    }
}

impl<'b, T: 'b, S: Index<usize, Output = Option<T>>> Cursor<'b, T> for SlotCursor<'b, T, S> {

    fn position(&self) -> Option<usize> {
        self.pos
    }

    fn current(&self) -> Option<&'b T> {
        self.pos.map(|p| self.slot(p))
    }

    fn seek(&mut self, p: usize) {
        assert!(p < self.nslots, "cursor: seek out of bounds");
        assert!(self.buf[p].is_some(), "cursor: seek to None");
        self.pos = Some(p);
    }

    fn next(&mut self) -> bool {
        let Some(p) = self.pos else { return false };
        for q in (p + 1)..self.nslots {
            if self.buf[q].is_some() {
                self.pos = Some(q);
                return true;
            }
        }
        self.pos = None;
        false
    }

    fn prev(&mut self) -> bool {
        let Some(p) = self.pos else { return false };
        for q in (0..p).rev() {
            if self.buf[q].is_some() {
                self.pos = Some(q);
                return true;
            }
        }
        false
    }

    fn first(&mut self) -> Option<usize> {
        for p in 0..self.nslots {
            if self.buf[p].is_some() {
                self.pos = Some(p);
                return Some(p);
            }
        }
        self.pos = None;
        None
    }

    fn last(&mut self) -> Option<usize> {
        for p in (0..self.nslots).rev() {
            if self.buf[p].is_some() {
                self.pos = Some(p);
                return Some(p);
            }
        }
        self.pos = None;
        None
    }
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

///bounded Vec-backed store. MAX_CAP bounds logical capacity.
pub(crate) struct VecStore<T, const MAX_CAP: usize> {
    buf:      Vec<Option<T>>,
    occupied: usize,
}

impl<T, const MAX_CAP: usize> VecStore<T, MAX_CAP> {
    const ASSERT_POW2: () = assert!(
        MAX_CAP != 0 && (MAX_CAP & (MAX_CAP - 1)) == 0,
        "MAX_CAP must be a power of two"
    );
}

impl<'a, T: Sized + 'a, const MAX_CAP: usize> Store<'a, T> for VecStore<T, MAX_CAP> {

    fn new() -> Self {
        Self { buf: Vec::new(), occupied: 0 }
    }

    fn get(&self, ptr: usize) -> &T {
        self.buf[ptr].as_ref().expect("store: None at occupied ptr")
    }

    fn get_mut(&mut self, ptr: usize) -> &mut T {
        self.buf[ptr].as_mut().expect("store: None at occupied ptr")
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
        match if dir { dual_scan_outward::<_, true>(buf, buf, pos.wrapping_sub(1), pos + 1, lcnt, rcnt) }
              else { dual_scan_outward::<_, false>(buf, buf, pos.wrapping_sub(1), pos + 1, lcnt, rcnt) }
        {
            NearestNone::Left(l) => {
                Some(NoneSlide { from: l, to: if !dir { pos - 1 } else { pos } })
            }
            NearestNone::Right(r) => Some(NoneSlide {
                from: r,
                to:   if dir { pos + 1 } else { pos },
            }),
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
                if dir { (pos, max) } else { (min, pos) }
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
                return Some(NoneSlide { from: pos + 1 + r, to: pos + 1 });
            }
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide { from: min + l, to: pos });
            }
            None
        } else {
            if lcnt > 0
                && let Some(l) = buf[min..pos].iter().rposition(|o| o.is_none())
            {
                return Some(NoneSlide { from: min + l, to: pos - 1 });
            }
            if rcnt > 0
                && let Some(r) = buf[pos + 1..max].iter().position(|o| o.is_none())
            {
                return Some(NoneSlide { from: pos + 1 + r, to: pos });
            }
            None
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.buf.swap(a, b)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        assert!(len < MAX_CAP, "max capacity");
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1).min(MAX_CAP);
            self.buf.reserve(target - c);
        }
        self.buf.insert(0, Some(v));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> usize {
        let len = self.buf.len();
        assert!(len < MAX_CAP, "max capacity");
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).max(1).min(MAX_CAP);
            self.buf.reserve(target - c);
        }
        self.buf.push(Some(v));
        self.occupied += 1;
        len
    }

    fn grow_front(&mut self, n: usize) {
        let len = self.buf.len();
        assert!(len + n <= MAX_CAP, "max capacity");
        self.buf.splice(0..0, (0..n).map(|_| None));
    }

    fn grow_back(&mut self, n: usize) -> usize {
        let len = self.buf.len();
        assert!(len + n <= MAX_CAP, "max capacity");
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
        let target = (c * 2).min(MAX_CAP).max(c + 1);
        if target > c {
            self.buf.reserve(target - c);
        }
    }

    fn spread(&mut self, offset: usize) {
        let len = self.buf.len();
        debug_assert!(offset < 2, "spread: offset must be 0 or 1");
        assert!(len * 2 <= MAX_CAP, "spread: exceeds max cap");
        if self.buf.capacity() < len * 2 {
            self.buf.reserve(len * 2 - self.buf.capacity());
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

    fn insert(&mut self, v: T, i: usize) {
        let slot = &mut self.buf[i];
        assert!(slot.is_none(), "insert into occupied");
        *slot = Some(v);
        self.occupied += 1;
    }

    fn remove(&mut self, i: usize) -> T {
        let slot = &mut self.buf[i];
        assert!(slot.is_some(), "remove empty");
        let v = slot.take().unwrap();
        self.occupied -= 1;
        v
    }

    fn split(&mut self, at: usize) -> Self {
        let right_count = self.buf.iter().skip(at).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at);
        self.occupied -= right_count;
        Self { buf: right, occupied: right_count }
    }

    fn split_and_rotate(&mut self, at: usize) -> Self {

        // odds-gap both halves: element at half-index i -> physical slot 2i+1 (even None).
        // the store doesn't know the address space (max representable addr != MAX_CAP in
        // general), so it just spreads by 2i+1; the block layer owns vptr/translator mapping.
        let cap = self.buf.capacity();
        let left_len = at;
        let right_len = self.buf.len() - at;
        assert!(
            right_len * 2 <= MAX_CAP && left_len * 2 <= MAX_CAP,
            "split_and_rotate: exceeds max cap"
        );
        let right_some = self.buf.iter().skip(at).filter(|s| s.is_some()).count();

        // new same-cap buf: right half (at+k) -> slot 2k+1, even None.
        let mut right_buf: Vec<Option<T>> = Vec::with_capacity(cap);
        right_buf.resize_with(right_len * 2, || None);
        for k in 0..right_len {
            right_buf[2 * k + 1] = self.buf[at + k].take();
        }

        // odds-gap left half in self: p -> 2p+1. reverse so destinations (>p, already vacated)
        // never clobber a not-yet-moved source; even slots end up None (sources are take'd).
        self.buf.resize_with(left_len * 2, || None);
        for p in (0..left_len).rev() {
            self.buf[2 * p + 1] = self.buf[p].take();
        }
        self.occupied -= right_some;
        Self { buf: right_buf, occupied: right_some }
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

    fn iter<'b>(&'b self) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }

    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where T: 'b {
        SlotCursor::new(&self.buf, self.buf.len())
    }

    fn slots<'b>(&self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where T: 'b {
        std::iter::empty::<&'b Option<T>>()
    }

    fn slice_iter<'b>(
        &'b self,
        _from: usize,
        _to: usize,
    ) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b,
    {
        std::iter::empty::<&'b Option<T>>()
    }

    fn max_capacity() -> usize {
        MAX_CAP
    }
}

impl<T, const MAX_CAP: usize> VecStore<T, MAX_CAP> {

    /// SAFETY: ptr must be in-bounds and occupied.
    pub(crate) unsafe fn get_unchecked(&self, ptr: usize) -> &T {
        unsafe { self.buf.get_unchecked(ptr).as_ref().unwrap_unchecked() }
    }

    /// SAFETY: ptr must be in-bounds and occupied.
    pub(crate) unsafe fn get_unchecked_mut(&mut self, ptr: usize) -> &mut T {
        unsafe { self.buf.get_unchecked_mut(ptr).as_mut().unwrap_unchecked() }
    }
}

///bounded VecDeque-backed store. MAX_CAP bounds logical capacity.
pub(crate) struct DequeStore<T, const MAX_CAP: usize> {
    buf:      VecDeque<Option<T>>,
    occupied: usize,
}

impl<T, const MAX_CAP: usize> DequeStore<T, MAX_CAP> {
    const ASSERT_POW2: () = assert!(
        MAX_CAP != 0 && (MAX_CAP & (MAX_CAP - 1)) == 0,
        "MAX_CAP must be a power of two"
    );
}

impl<'a, T: Sized + 'a, const MAX_CAP: usize> Store<'a, T> for DequeStore<T, MAX_CAP> {

    fn new() -> Self {
        Self { buf: VecDeque::new(), occupied: 0 }
    }

    fn get(&self, ptr: usize) -> &T {
        self.buf[ptr].as_ref().expect("store: None at occupied ptr")
    }

    fn get_mut(&mut self, ptr: usize) -> &mut T {
        self.buf[ptr].as_mut().expect("store: None at occupied ptr")
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
                    true => dual_scan_outward::<_, true>(front, back, pos.wrapping_sub(1), pos + 1, pos - min, fmax.saturating_sub(pos + 1)),
                    false => dual_scan_outward::<_, false>(front, back, pos.wrapping_sub(1), pos + 1, pos - min, fmax.saturating_sub(pos + 1)),
                };
                match scan(front, front) {
                    NearestNone::Left(l) => {
                        Some(NoneSlide { from: l, to: if !dir { pos - 1 } else { pos } })
                    }
                    NearestNone::Right(r) => Some(NoneSlide {
                        from: r,
                        to:   if dir { pos + 1 } else { pos },
                    }),

                    //front exhausted within budget: any None in back is right of pos.
                    NearestNone::NotFound => {
                        back[0..max.saturating_sub(fl)].iter().position(|i| i.is_none()).map(|x| {
                            let r = x + fl;
                            NoneSlide { from: r, to: if dir { pos + 1 } else { pos } }
                        })
                    }
                }
            }
            Equal => {
                //pos = fl, occupied by contract (buf[fl] = back[0]); left = front
                //[min, fl), right = back (0, max-fl).
                debug_assert!(!back.is_empty() && back[0].is_some());
                let bcnt = max.saturating_sub(fl);
                let scan = |front, back| match dir {
                    true => dual_scan_outward::<_, true>(front, back, fl.wrapping_sub(1), 1, fl - min, bcnt.saturating_sub(1)),
                    false => dual_scan_outward::<_, false>(front, back, fl.wrapping_sub(1), 1, fl - min, bcnt.saturating_sub(1)),
                };
                match scan(front, back) {
                    NearestNone::Left(p) => {
                        Some(NoneSlide { from: p, to: if !dir { pos - 1 } else { pos } })
                    }
                    NearestNone::Right(p) => {
                        let r = p + fl;
                        Some(NoneSlide { from: r, to: if dir { pos + 1 } else { pos } })
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
                    true => dual_scan_outward::<_, true>(front, back, fpos.wrapping_sub(1), fpos + 1, fpos - fmin, fmax.saturating_sub(fpos + 1)),
                    false => dual_scan_outward::<_, false>(front, back, fpos.wrapping_sub(1), fpos + 1, fpos - fmin, fmax.saturating_sub(fpos + 1)),
                };
                match scan(back, back) {
                    NearestNone::Left(l) => {
                        let abs = l + fl;
                        Some(NoneSlide { from: abs, to: if !dir { pos - 1 } else { pos } })
                    }
                    NearestNone::Right(r) => {
                        let abs = r + fl;
                        Some(NoneSlide { from: abs, to: if dir { pos + 1 } else { pos } })
                    }

                    //back exhausted within budget: any None in front is left of pos.
                    NearestNone::NotFound => front[min.min(fl)..fl]
                        .iter()
                        .rev()
                        .position(|o| o.is_none())
                        .map(|p| {
                            let abs = fl - p - 1;
                            NoneSlide { from: abs, to: if !dir { pos - 1 } else { pos } }
                        }),
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
                if dir { (pos, max) } else { (min, pos) }
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
                    return Some(fl + l);
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
                return Some(NoneSlide { from: r, to: pos + 1 });
            }
            if let Some(l) = scan_left() {
                return Some(NoneSlide { from: l, to: pos });
            }
            None
        } else {
            if let Some(l) = scan_left() {
                return Some(NoneSlide { from: l, to: pos - 1 });
            }
            if let Some(r) = scan_right() {
                return Some(NoneSlide { from: r, to: pos });
            }
            None
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.buf.swap(a, b)
    }

    fn push_front(&mut self, v: T) {
        let len = self.buf.len();
        assert!(len < MAX_CAP, "max capacity");
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).min(MAX_CAP).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_front(Some(v));
        self.occupied += 1;
    }

    fn push_back(&mut self, v: T) -> usize {
        let len = self.buf.len();
        assert!(len < MAX_CAP, "max capacity");
        if len == self.buf.capacity() {
            let c = self.buf.capacity();
            let target = (c * 2).min(MAX_CAP).max(c + 1);
            let _ = self.buf.reserve(target - c);
        }
        self.buf.push_back(Some(v));
        self.occupied += 1;
        len
    }

    fn grow_front(&mut self, n: usize) {
        let len = self.buf.len();
        assert!(len + n <= MAX_CAP, "max capacity");
        for _ in 0..n {
            self.buf.push_front(None);
        }
    }

    fn grow_back(&mut self, n: usize) -> usize {
        let len = self.buf.len();
        assert!(len + n <= MAX_CAP, "max capacity");
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
        let target = (c * 2).min(MAX_CAP).max(c + 1);
        if target > c {
            let _ = self.buf.reserve(target - c);
        }
    }

    fn spread(&mut self, offset: usize) {
        let len = self.buf.len();
        debug_assert!(offset < 2, "spread: offset must be 0 or 1");
        assert!(len * 2 <= MAX_CAP, "spread: exceeds max cap");
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

    fn insert(&mut self, v: T, i: usize) {
        let slot = &mut self.buf[i];
        assert!(slot.is_none(), "insert into occupied");
        *slot = Some(v);
        self.occupied += 1;
    }

    fn remove(&mut self, i: usize) -> T {
        let slot = &mut self.buf[i];
        assert!(slot.is_some(), "remove empty");
        let v = slot.take().unwrap();
        self.occupied -= 1;
        v
    }

    fn split(&mut self, at: usize) -> Self {
        let right_count = self.buf.iter().skip(at).filter(|s| s.is_some()).count();
        let right = self.buf.split_off(at);
        self.occupied -= right_count;
        Self { buf: right, occupied: right_count }
    }

    fn split_and_rotate(&mut self, at: usize) -> Self {

        // odds-gap both halves: element at half-index i -> physical slot 2i+1 (even None).
        // the store doesn't know the address space (max representable addr != MAX_CAP in
        // general), so it just spreads by 2i+1; the block layer owns vptr/translator mapping.
        let cap = self.buf.capacity();
        let left_len = at;
        let right_len = self.buf.len() - at;
        assert!(
            right_len * 2 <= MAX_CAP && left_len * 2 <= MAX_CAP,
            "split_and_rotate: exceeds max cap"
        );
        let right_some = self.buf.iter().skip(at).filter(|s| s.is_some()).count();

        // new same-cap buf: right half (at+k) -> slot 2k+1, even None.
        let mut right_buf: VecDeque<Option<T>> = VecDeque::with_capacity(cap);
        right_buf.resize_with(right_len * 2, || None);
        for k in 0..right_len {
            right_buf[2 * k + 1] = self.buf[at + k].take();
        }

        // odds-gap left half in self: p -> 2p+1. reverse so destinations (>p, already vacated)
        // never clobber a not-yet-moved source; even slots end up None (sources are take'd).
        self.buf.resize_with(left_len * 2, || None);
        for p in (0..left_len).rev() {
            self.buf[2 * p + 1] = self.buf[p].take();
        }
        self.occupied -= right_some;
        Self { buf: right_buf, occupied: right_some }
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

    fn iter<'b>(&'b self) -> impl DoubleEndedIterator<Item = &'b T> + ExactSizeIterator<Item = &'b T> + 'b
    where T: 'b {
        SomeIter { inner: self.buf.iter(), remaining: self.occupied }
    }

    fn cursor<'b>(&'b self) -> impl Cursor<'b, T> + 'b
    where T: 'b {
        SlotCursor::new(&self.buf, self.buf.len())
    }

    fn slots<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where T: 'b {
        std::iter::empty::<&'b Option<T>>()
    }

    fn slice_iter<'b>(
        &'b self,
        _from: usize,
        _to: usize,
    ) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b,
    {
        std::iter::empty::<&'b Option<T>>()
    }

    fn max_capacity() -> usize {
        MAX_CAP
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
