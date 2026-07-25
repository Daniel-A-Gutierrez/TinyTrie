use std::cmp::Ordering::*;
use std::{collections::VecDeque, ops::Range};
///realistically this is a wrapper over vec<Option<T>> and vecdeque<Option<T>> that limits max cap and provides
///access/shift semantics
///address translation
///dumb insertion
pub(crate) trait Store<'a, T: Sized + 'a>: Sized {
    ///in-bounds occupied slot. bounds-checks; panics if the slot is None (contract violation).
    fn get(&self, ptr: usize) -> &T;
    fn get_mut(&mut self, ptr: usize) -> &mut T;
    //move a contiguous series of Some elements right or left 1 onto a None space.
    fn shift_items(&mut self, src: ScanResult);
    //find the closest None slot to anchor within max_dist
    fn find_slot<const DIR: bool>(&self, anchor: usize, max_dist: usize) -> Option<ScanResult>;
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
    ///doubles len, moves elements at i to 2*i
    fn spread(&mut self);
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
    fn iter<'b>(&self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b;
    fn slots<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b;
    fn slice_iter<'b>(
        &'b self,
        from: usize,
        to: usize,
    ) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b;
    fn max_capacity() -> usize; //the maximum capacity of the store type.
    fn new() -> Self;
}
pub enum Direction {
    Left,
    Right,
}
impl Direction {
    fn to_bool(&self) -> bool {
        match self {
            Self::Left => false,
            Self::Right => true,
        }
    }
}
///represents a contiguous range of Some(thing) in a store, ending at a None.
///not ordered, from may be > to, but from!=0.
pub struct ScanResult {
    pub(crate) pos:     usize,
    pub(crate) nearest: usize,
}
impl ScanResult {
    fn new(pos: usize, nearest: usize) -> Self {
        ScanResult { pos, nearest }
    }
    fn distance(&self) -> usize {
        self.nearest.abs_diff(self.pos)
    }
    fn direction(&self) -> Direction {
        if self.nearest > self.pos { Direction::Right } else { Direction::Left }
    }
}
pub enum NearestNone {
    Left(usize),
    Right(usize),
    NotFound,
}
impl NearestNone {
    fn to_position(self) -> Option<usize> {
        match self {
            Self::Left(pos) => Some(pos),
            Self::Right(pos) => Some(pos),
            Self::NotFound => None,
        }
    }
}
///caller guarantees ranges are in bounds of their respective slices
///scan 2 ranges in 2 slices, returning the index of the first found None wrapped in which slice it belongs to, biased left.
///left/down is scanned in reverse.
///Direction tie breaks. false for left true for right.
#[inline]
fn dual_scan_outward<T: Sized, const D: bool>(
    left: &[Option<T>],
    right: &[Option<T>],
    down: Range<usize>,
    up: Range<usize>,
) -> NearestNone {
    let mut lefti = down.end;
    let mut righti = up.start;
    let rlen = up.end - (up.start);
    let llen = down.end - (down.start);
    let mlen = rlen.min(llen);
    for _i in 0..mlen {
        let l = &left[lefti];
        let r = &right[righti];
        match (l.is_none(), r.is_none()) {
            (true, true) => {
                //equidistant: tie-break by D. false=>left, true=>right.
                return if D { NearestNone::Right(righti) } else { NearestNone::Left(lefti) };
            }
            (true, false) => return NearestNone::Left(lefti),
            (false, true) => return NearestNone::Right(righti),
            (false, false) => {}
        }
        lefti -= 1;
        righti += 1;
    }
    for _ in mlen..llen {
        if let None = left[lefti] {
            return NearestNone::Left(lefti);
        }
        lefti -= 1;
    }
    for _ in mlen..rlen {
        if let None = right[righti] {
            return NearestNone::Right(righti);
        }
        righti += 1;
    }
    return NearestNone::NotFound;
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
    fn shift_items(&mut self, _src: ScanResult) {
        todo!()
    }
    fn find_slot<const DIR: bool>(&self, anchor: usize, max_dist: usize) -> Option<ScanResult> {
        let buf = self.buf.as_slice();
        let max = (u32::MAX as usize).min(self.buf.len()).min(anchor + max_dist);
        let min = anchor.saturating_sub(max_dist);
        dual_scan_outward::<_, DIR>(buf, buf, min..anchor, anchor..max)
            .to_position()
            .map(|p| ScanResult::new(anchor, p))
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
            let target = (c * 2).min(MAX_CAP).max(c + 1);
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
    fn spread(&mut self) {
        let len = self.buf.len();
        assert!(len * 2 <= MAX_CAP, "spread: exceeds max cap");
        if self.buf.capacity() < len * 2 {
            self.buf.reserve(len * 2 - self.buf.capacity());
        }
        // one pass: take src i -> value to dst=2i, None to dst+1.
        let base = self.buf.as_mut_ptr();
        for i in (0..len).rev() {
            let dst = 2 * i;
            // SAFETY: i in [0,len) init. dst<len is init (None, take'd by an earlier higher-i
            // iter); dst>=len is uninit spare. dst+1>=len is uninit spare. ~2.5*len writes.
            let v = unsafe { (*base.add(i)).take() };
            unsafe {
                if dst < len {
                    *base.add(dst) = v;
                } else {
                    base.add(dst).write(v);
                }
                if dst + 1 >= len {
                    base.add(dst + 1).write(None);
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
    fn iter<'b>(&self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b,
    {
        std::iter::empty::<&'b T>()
    }
    fn slots<'b>(&self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b,
    {
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
    fn shift_items(&mut self, _src: ScanResult) {
        todo!()
    }
    fn find_slot<const DIR: bool>(&self, pos: usize, budget: usize) -> Option<ScanResult> {
        let (front, back) = self.buf.as_slices();
        let max = (u32::MAX as usize).min(self.buf.len()).min(pos + budget);
        let min = pos.saturating_sub(budget);
        //keypoints - min , boundary, pos,pos+1, max . boundary can lie at any relative position.
        match pos.cmp(&front.len()) {
            Less => {
                let fmax = max.min(front.len());
                dual_scan_outward::<_, DIR>(front, front, min..pos, pos..fmax)
                    .to_position()
                    .or_else(|| {
                        back[0..max - front.len()]
                            .into_iter()
                            .position(|i| i.is_none())
                            .map(|x| x + front.len())
                    })
                    .map(|f| ScanResult::new(pos, f))
            }
            Equal => match dual_scan_outward::<_, DIR>(
                front,
                back,
                min..front.len(),
                0..max - front.len(),
            ) {
                NearestNone::Left(p) => Some(ScanResult::new(pos, p)),
                NearestNone::Right(p) => Some(ScanResult::new(pos, p + front.len())),
                NearestNone::NotFound => None,
            },
            Greater => {
                let fmin = min.saturating_sub(front.len());
                let fmax = max - front.len();
                let fpos = pos - front.len();
                dual_scan_outward::<_, DIR>(back, back, fmin..fpos, fpos..fmax)
                    .to_position()
                    .map(|x| ScanResult::new(pos, x - front.len()))
                    .or_else(|| {
                        front[min.min(front.len())..front.len()]
                            .into_iter()
                            .rev()
                            .position(|o| o.is_none())
                            .map(|p| ScanResult::new(pos, front.len() - p - 1))
                    })
            }
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
    fn spread(&mut self) {
        let len = self.buf.len();
        assert!(len * 2 <= MAX_CAP, "spread: exceeds max cap");
        let mid = len / 2;
        // phase1: take upper half [mid,len), push each then a None. upper half lands
        // in final [len,2*len) (evens=value, odds=None, no wasted even-slot None-init);
        // [mid,len) becomes None (space for phase2). ~1.5*len writes.
        for i in mid..len {
            let v = self.buf[i].take();
            self.buf.push_back(v);
            self.buf.push_back(None);
        }
        // phase2: spread lower half [0,mid) over [0,len); space [mid,len) is None.
        // reverse: 2*j>=j so slot 2*j is vacated (2*j<mid) or None (>=mid). safe.
        // contig -> index the slice (skips deque's per-access (head+i)%cap); wrapped
        // -> make_contiguous's O(n) linearize is a net loss, so eat the deque-index cost.
        if self.buf.as_mut_slices().1.is_empty() {
            let s = self.buf.as_mut_slices().0;
            for j in (0..mid).rev() {
                let v = s[j].take();
                s[2 * j] = v;
            }
        } else {
            for j in (0..mid).rev() {
                let v = self.buf[j].take();
                self.buf[2 * j] = v;
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
    fn iter<'b>(&self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b,
    {
        std::iter::empty::<&'b T>()
    }
    fn slots<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Option<T>> + 'b
    where
        T: 'b,
    {
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
