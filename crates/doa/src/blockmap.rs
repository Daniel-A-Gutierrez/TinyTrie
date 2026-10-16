//!BlockMap: BlockSet's substrate with a value riding beside every key —
//!two parallel `MaybeUninit` vecs (keys, values) under one shared mask.
//!moves are the blockset ladder (spot / push_back / gather / turn), each
//!relocating the (key, value) pair; the events are the same `Deltas`
//!protocol (addr names — value slots are nameless, a name finds the pair).
//!order: pos order == key order — pos 0 = min key (Asc) or max (Desc);
//!the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): key-ordered walk over live pairs,
//!blockset::Cursor's machine (see blockset).

use std::cmp::Ordering;
use std::mem::MaybeUninit;

use crate::blockset::{CursorSlot, Deltas, FREE_STRIDE, L2_BYTES, TURN_DEN, TURN_NUM};
use crate::blockset::NoneSlide;
use crate::translator::Translator;
use crate::index::*;
use crate::mask::{Mask, SetBits, SetBitsRev};

///mask covers keys.len() slots; bit ⇔ slot initialized (key + value).
pub struct BlockMap<K, V, A> {
    translator: Translator<A>,
    keys:      Vec<MaybeUninit<K>>,
    vals:      Vec<MaybeUninit<V>>,
    mask:      Mask,
    len:       usize,
    ///running sum of placed slot positions — the reversal tier's signal
    pos_sum:   u64,
}

impl<K, V, A> Drop for BlockMap<K, V, A> {
    fn drop(&mut self) {
        for p in self.mask.iter_set() {
            //mask ⇔ init invariant
            unsafe {
                self.keys[p].assume_init_drop();
                self.vals[p].assume_init_drop();
            }
        }
    }
}

impl<K: Ord, V, A: UnsignedNum> BlockMap<K, V, A> {
    pub fn new() -> Self {
        BlockMap {
            translator: Translator::new(),
            keys:      Vec::new(),
            vals:      Vec::new(),
            mask:      Mask::new(),
            len:       0,
            pos_sum:   0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn cap(&self) -> usize {
        self.keys.len()
    }

    pub fn max_capacity() -> usize {
        A::MAX.as_usize().saturating_add(1)
    }

    ///None if addr names no live entry (never-minted names can't alias a
    ///live slot — a2p is injective; STALE pre-delta names can, see
    ///subtle_bugs §17)
    pub fn get(&self, addr: A) -> Option<(&K, &V)> {
        let p = self.translator.a2p(addr);
        (p.0 < self.keys.len() && self.mask.get(p.0)).then(|| unsafe { self.pair(p.0) })
    }

    ///value by key — the primary read; None if k absent
    pub fn value(&self, k: &K) -> Option<&V> {
        self.position(k).ok().map(|p| unsafe { self.pair(p.0).1 })
    }

    ///value by key, mutably
    pub fn value_mut(&mut self, k: &K) -> Option<&mut V> {
        let p = self.position(k).ok()?;
        Some(unsafe { self.vals[p.0].assume_init_mut() })
    }

    ///overwrite the value of the entry named addr; false if not live.
    ///drops the old value — plain slot assignment would leak it
    pub fn set_value(&mut self, addr: A, v: V) -> bool {
        let p = self.translator.a2p(addr);
        if p.0 < self.keys.len() && self.mask.get(p.0) {
            //mask ⇔ init invariant upholds the drop
            unsafe { self.vals[p.0].assume_init_drop() };
            self.vals[p.0] = MaybeUninit::new(v);
            return true;
        }
        false
    }

    ///live-checked value read by POS (pub(crate): the pos-space deltas
    ///path — `iter_pos` consumers; the flip fires pre-record so recorded
    ///positions match the emitter's slots)
    pub(crate) fn value_at_pos(&self, p: Pos) -> Option<&V> {
        (p.0 < self.keys.len() && self.mask.get(p.0)).then(|| unsafe { self.vals[p.0].assume_init_ref() })
    }

    ///the slot's name, p2a (pub(crate): pos-space consumers write held names)
    pub(crate) fn name_at(&self, p: Pos) -> A {
        self.translator.p2a(p)
    }

    #[inline]
    pub fn find(&self, k: &K) -> Option<A> {
        //beyond either extreme → absent, no bsearch — the common
        //sequential probe (append/prepend dup checks land past the tail)
        if self.len > 0 {
            let tail = self.key_tail();
            let beyond = if self.translator.reversed() { *k < *tail } else { *tail < *k };
            if beyond {
                return None;
            }
            //len > 0 ⇒ a first live slot exists
            let head = unsafe { self.key(self.mask.next_set(0).unwrap()) };
            let before = if self.translator.reversed() { *head < *k } else { *k < *head };
            if before {
                return None;
            }
        }
        match self.position(k) {
            Ok(p) => Some(self.translator.p2a(p)),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    ///first→last over live (key, value) pairs, key order
    pub fn iter(&self) -> Iter<'_, K, V, A> {
        //direction picked once (see blockset::Iter)
        if self.translator.reversed() {
            Iter::Desc { bits: self.mask.iter_set_rev(), map: self }
        } else {
            Iter::Asc { bits: self.mask.iter_set(), map: self }
        }
    }

    ///cursor at the first key ≥ k, or the walk's end if none
    pub fn seek(&self, k: &K) -> Cursor<'_, K, V, A> {
        let mut cursor = Cursor::new(self);
        cursor.seek(k);
        cursor
    }

    ///cursor at the first key > k — k itself lands in a downward walk
    pub fn seek_after(&self, k: &K) -> Cursor<'_, K, V, A> {
        let mut cursor = Cursor::new(self);
        cursor.seek_after(k);
        cursor
    }

    ///first live slot at/after the pos boundary `from`, in KEY order (see
    ///blockset::key_bound)
    fn key_bound(&self, from: usize) -> Option<usize> {
        if self.translator.reversed() {
            self.mask.prev_set(from)
        } else {
            self.mask.next_set(from)
        }
    }

    ///deltas must be empty (applied + cleared) — panics if pending. Err
    ///returns (k, v) (dup, or out of names)
    pub fn insert(&mut self, k: K, v: V, deltas: &mut Deltas<A>) -> Result<A, (K, V)> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.keys.is_empty() {
            return Ok(self.insert_first(k, v));
        }
        if self.len >= Self::max_capacity() {
            return Err((k, v));
        }
        //flip check BEFORE the record — no translator mutation follows the
        //event record inside insert (pos-space consumers rely on that;
        //see blockset::insert)
        self.maybe_reverse();
        let Some(spot) = self.spot(&k) else {
            return Err((k, v));
        };
        //sequential at the back: extend instead of sliding
        if spot.0 == self.keys.len() && self.keys.len() < Self::max_capacity() {
            let a = self.push_back(k, v, spot);
            return Ok(a);
        }
        else {
            let p = self.open(spot, deltas);
            let a = self.place(k, v, p);
            Ok(a)
        }
    }

    ///in-place reversal: pos 0 = max key, names follow the slots (the
    ///translator absorbs it — no deltas; a pending event stays valid)
    fn reverse(&mut self) {
        self.keys.reverse();
        self.vals.reverse();
        self.mask.reverse();
        self.translator = self.translator.reverse(self.keys.len());
        let len = self.len as u64;
        //reflect the bias signal around len (see blockset::reverse)
        self.pos_sum = len.wrapping_mul(len).wrapping_sub(self.pos_sum);
    }

    ///flip a front-biased insert stream so future inserts land at the
    ///tail (see blockset::maybe_reverse)
    fn maybe_reverse(&mut self) {
        if self.len > 4 * FREE_STRIDE {
            let len = self.len as u64;
            if self.pos_sum.wrapping_mul(8) < len.wrapping_mul(len) {
                self.reverse();
            }
        }
    }

    ///insertion spot, or None on dup
    fn spot(&self, k: &K) -> Option<Pos> {
        if self.len > 0 {
            let tail = self.key_tail();
            let beyond = if self.translator.reversed() { k < tail } else { tail < k };
            if beyond {
                return Some(Pos(self.mask.last_set().unwrap() + 1));
            }
        }
        self.bsearch(k).err()
    }

    ///vec-like first allocation: 4 slots, no translator turn until it fills
    fn insert_first(&mut self, k: K, v: V) -> A {
        self.keys = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.vals = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.mask = Mask::with_bits(4);
        self.place(k, v, Pos(0))
    }

    ///extend the allocation and place the pair at the edge — dense, holes
    ///never minted here. caller guarantees spot == cap < max_capacity
    fn push_back(&mut self, k: K, v: V, spot: Pos) -> A {
        self.push_slot();
        self.place(k, v, spot)
    }

    ///open a slot at/near `spot`: turns and gathers, escalating until a
    ///hole opens (see blockset::open)
    fn open(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let mut spot = spot;
        //preemptive turn past 7/8 (see blockset::open)
        if self.len.wrapping_mul(TURN_DEN) > self.keys.len().wrapping_mul(TURN_NUM)
            && self.keys.len() - spot.0 > FREE_STRIDE
        {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            }
        }
        if let Some(p) = self.gather_slots(spot, deltas) {
            return p;
        }
        //mask full: tail push within FREE_STRIDE, else a turn (see
        //blockset::open)
        if self.keys.len() - spot.0 <= FREE_STRIDE && self.keys.len() < Self::max_capacity() {
            self.push_slot();
        } else {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            } else {
                self.push_slot();
            }
        }
        self.gather_slots(spot, deltas).expect("mask had a hole minted")
    }

    ///k's spot under a completed turn (see blockset::turned_spot)
    fn turned_spot(&self, old: Translator<A>, spot: Pos) -> Pos {
        self.translator.a2p(old.p2a(spot))
    }

    ///write the pair at an open slot, mark live, mint its name; the slot
    ///position joins the running insert-position sum
    fn place(&mut self, k: K, v: V, p: Pos) -> A {
        self.keys[p.0] = MaybeUninit::new(k);
        self.vals[p.0] = MaybeUninit::new(v);
        self.mask.set(p.0, true);
        self.len += 1;
        self.pos_sum = self.pos_sum.wrapping_add(p.0 as u64);
        self.translator.p2a(p)
    }

    pub fn remove(&mut self, addr: A) -> Option<(K, V)> {
        self.remove_pos(self.translator.a2p(addr))
    }

    ///take by key
    pub fn remove_key(&mut self, k: &K) -> Option<(K, V)> {
        self.position(k).ok().and_then(|p| self.remove_pos(p))
    }

    ///None if addr names no live entry (stale-name hazard: get/remove_pos)
    fn remove_pos(&mut self, pos: Pos) -> Option<(K, V)> {
        if pos.0 < self.keys.len() && self.mask.get(pos.0) {
            self.mask.set(pos.0, false);
            self.len -= 1;
            //mask ⇔ init invariant
            return Some(unsafe {
                (self.keys[pos.0].assume_init_read(), self.vals[pos.0].assume_init_read())
            });
        }
        None
    }

    ///bsearch over live slots — the raw boundary (see blockset::bsearch;
    ///the shape split measures over K's footprint)
    fn bsearch(&self, k: &K) -> Result<Pos, Pos> {
        let big = self.keys.len().wrapping_mul(std::mem::size_of::<K>()) > L2_BYTES;
        match (self.translator.reversed(), big) {
            (true, true) => self.search_spec::<true>(k),
            (true, false) => self.search_cmov::<true>(k),
            (false, true) => self.search_spec::<false>(k),
            (false, false) => self.search_cmov::<false>(k),
        }
    }

    ///speculating search — early exit, branch per probe (see
    ///blockset::search_spec)
    fn search_spec<const REV: bool>(&self, k: &K) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (0, self.keys.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    let ord = if REV { k.cmp(probe) } else { probe.cmp(k) };
                    match ord {
                        Ordering::Less => lo = p + 1,
                        Ordering::Greater => hi = p,
                        Ordering::Equal => return Ok(Pos(p)),
                    }
                }
            }
        }
        Err(Pos(lo))
    }

    ///fixed cmov search (see blockset::search_cmov)
    fn search_cmov<const REV: bool>(&self, k: &K) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (0, self.keys.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    let ord = if REV { k.cmp(probe) } else { probe.cmp(k) };
                    if ord == Ordering::Less {
                        lo = p + 1;
                    } else {
                        hi = p;
                    }
                }
            }
        }
        if let Some(p) = self.next_live(lo, self.keys.len()) {
            if unsafe { self.key(p) } == k {
                return Ok(Pos(p));
            }
        }
        Err(Pos(lo))
    }

    ///locate k: Ok(p) = the duplicate at p; Err = the first LIVE
    ///element past k's run, or cap past the end
    fn position(&self, k: &K) -> Result<Pos, Pos> {
        match self.bsearch(k) {
            Ok(p) => Ok(p),
            Err(spot) => Err(self.next_live(spot.0, self.keys.len()).map(Pos).unwrap_or(spot)),
        }
    }

    #[inline]
    fn next_live(&self, from: usize, bound: usize) -> Option<usize> {
        self.mask.next_set(from).filter(|p| *p < bound)
    }

    ///raw live-slot reads
    ///
    /// # Safety
    ///
    /// p must name a live slot — mask ⇔ init
    unsafe fn key(&self, p: usize) -> &K {
        unsafe { self.keys[p].assume_init_ref() }
    }

    /// # Safety
    ///
    /// p must name a live slot — mask ⇔ init
    unsafe fn pair(&self, p: usize) -> (&K, &V) {
        unsafe { (self.keys[p].assume_init_ref(), self.vals[p].assume_init_ref()) }
    }

    ///key at the physical tail (max Asc, min Desc); panics if len == 0
    fn key_tail(&self) -> &K {
        unsafe { self.key(self.mask.last_set().expect("live")) }
    }

    ///open a slot at/near `spot`; None ⇒ hole-free mask. the opened slot
    ///arrives as one `Delta` event (NoneSlide or Gather — see blockset)
    fn gather_slots(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Option<Pos> {
        //k's slot already open — no fetch, no slide (see blockset)
        if spot.0 < self.cap() && !self.mask.get(spot.0) {
            deltas.record_none_slide(self.translator, spot, spot);
            return Some(spot);
        }
        //the nearest hole's distance sets the fetch (see blockset)
        let hole = self.mask.nearest_clear(spot.0)?;
        let right = hole > spot.0;
        let open = if right { spot.0 } else { spot.0 - 1 };
        if hole.abs_diff(spot.0) < FREE_STRIDE {
            self.slide_hole(hole, open);
            deltas.record_none_slide(self.translator, Pos(hole), Pos(open));
            return Some(Pos(open));
        }
        let holes = self.fetch_holes(spot, right, 1 + hole.abs_diff(spot.0) / FREE_STRIDE);
        self.compact_gather(spot, open, right, &holes);
        deltas.record_gather(
            self.translator,
            holes
                .iter()
                .enumerate()
                .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                .collect(),
        );
        Some(Pos(open))
    }

    ///one plain slide: hole `from` → `to`, the pair-run between in
    ///lockstep
    fn slide_hole(&mut self, from: usize, to: usize) {
        unsafe {
            let (kp, vp) = (self.keys.as_mut_ptr(), self.vals.as_mut_ptr());
            if from > to {
                std::ptr::copy(kp.add(to), kp.add(to + 1), from - to);
                std::ptr::copy(vp.add(to), vp.add(to + 1), from - to);
            } else {
                std::ptr::copy(kp.add(from + 1), kp.add(from), to - from);
                std::ptr::copy(vp.add(from + 1), vp.add(from), to - from);
            }
        }
        //the run between is fully live (nearest hole) — one shared two-bit
        //mask update
        self.mask.set(from, true);
        self.mask.set(to, false);
    }

    ///the `want` nearest holes on one side of `spot`, nearest-first (see
    ///blockset::fetch_holes)
    fn fetch_holes(&self, spot: Pos, right: bool, want: usize) -> Vec<usize> {
        let mut holes = Vec::with_capacity(want);
        if right {
            let mut cursor = spot.0;
            while holes.len() < want
                && let Some(h) = self.mask.next_clear(cursor)
            {
                holes.push(h);
                cursor = h + 1;
            }
        } else {
            let mut cursor = spot.0 - 1;
            while holes.len() < want {
                let Some(h) = self.mask.prev_clear(cursor) else { break };
                holes.push(h);
                if h == 0 {
                    break;
                }
                cursor = h - 1;
            }
        }
        holes
    }

    ///gather compaction, one pass, both vecs (see blockset::compact_gather
    ///for the segment choreography)
    fn compact_gather(&mut self, spot: Pos, open: usize, right: bool, holes: &[usize]) {
        let w = holes.len();
        unsafe {
            let (kp, vp) = (self.keys.as_mut_ptr(), self.vals.as_mut_ptr());
            //segment j shifts by its crossing count; far segments copy
            //first so no live source is read after being overwritten
            if right {
                for j in (0..w).rev() {
                    let (lo, len) = if j == 0 {
                        (spot.0, holes[0] - spot.0)
                    } else {
                        (holes[j - 1] + 1, holes[j] - holes[j - 1] - 1)
                    };
                    let shift = w - j;
                    std::ptr::copy(kp.add(lo), kp.add(lo + shift), len);
                    std::ptr::copy(vp.add(lo), vp.add(lo + shift), len);
                }
            } else {
                for k in (0..w).rev() {
                    let (lo, len) = if k == 0 {
                        (holes[0] + 1, spot.0 - 1 - holes[0])
                    } else {
                        (holes[k] + 1, holes[k - 1] - holes[k] - 1)
                    };
                    let shift = w - k;
                    std::ptr::copy(kp.add(lo), kp.add(lo - shift), len);
                    std::ptr::copy(vp.add(lo), vp.add(lo - shift), len);
                }
            }
        }
        for &h in holes {
            self.mask.set(h, true);
        }
        //the vacated target range clears; sets first (see blockset)
        if right {
            for t in open..open + w {
                self.mask.set(t, false);
            }
        } else {
            for t in open + 1 - w..=open {
                self.mask.set(t, false);
            }
        }
    }

    ///rotation turn (in-place): cap doubles, every live pair relocates to
    ///`a2p_new(name)` — names preserved by construction. false ⇒ refused
    ///(see blockset::grow_rotate)
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.keys.len();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        if old_cap > A::MIDPOINT.as_usize()
            || self.translator.rotation() >= A::BIT_WIDTH as u32
        {
            return false;
        }
        let new_tr = self.translator.rotate();
        self.keys.reserve(old_cap);
        self.keys.extend((old_cap..new_cap).map(|_| MaybeUninit::uninit()));
        self.vals.reserve(old_cap);
        self.vals.extend((old_cap..new_cap).map(|_| MaybeUninit::uninit()));
        //descending pass, both vecs — every target written before its
        //source is read (see blockset::grow_rotate)
        for p in (0..old_cap).rev() {
            if self.mask.get(p) {
                let q = new_tr.a2p(self.translator.p2a(Pos(p)));
                debug_assert!(q.0 >= p, "turn must not move an element down");
                self.keys[q.0] = std::mem::replace(&mut self.keys[p], MaybeUninit::uninit());
                self.vals[q.0] = std::mem::replace(&mut self.vals[p], MaybeUninit::uninit());
            }
        }
        //one shared mask half
        self.mask.rotate_mask(
            self.translator.offset().as_usize(),
            A::MIDPOINT.as_usize(),
            self.translator.reversed(),
        );
        self.translator = new_tr;
        self.pos_sum = self.pos_sum.wrapping_mul(2);
        true
    }

    ///extend both vecs by one uninit slot (mask clear) — the Vecs' own
    ///doubling carries reallocation. panics at the name-space ceiling
    fn push_slot(&mut self) {
        assert!(self.keys.len() < Self::max_capacity(), "name space exhausted");
        self.keys.push(MaybeUninit::uninit());
        self.vals.push(MaybeUninit::uninit());
        self.mask.resize(self.keys.len());
    }
}

///`BlockMap::iter`'s return type — direction picked at construction
pub enum Iter<'a, K, V, A> {
    Asc { bits: SetBits<'a>, map: &'a BlockMap<K, V, A> },
    Desc { bits: SetBitsRev<'a>, map: &'a BlockMap<K, V, A> },
}

impl<'a, K: Ord, V, A: UnsignedNum> Iterator for Iter<'a, K, V, A> {
    type Item = (&'a K, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            //mask ⇔ init invariant upholds the reads
            Iter::Asc { bits, map } => bits.next().map(|p| unsafe { map.pair(p) }),
            Iter::Desc { bits, map } => bits.next().map(|p| unsafe { map.pair(p) }),
        }
    }
}

///key-ordered bidirectional walk over live pairs — blockset::Cursor's
///machine, one (key, value) pair per yield, steps on a cached mask word
///(see blockset::Cursor). invalidated by any mutation — re-seek
pub struct Cursor<'a, K, V, A> {
    map:   &'a BlockMap<K, V, A>,
    ///baked at construction (see blockset::Cursor)
    rev:   bool,
    ///the next-to-yield slot, or the forward ghost
    state: CursorSlot,
    ///state At(p) ⇒ p's mask word; FwdEnd leaves it stale (land reloads)
    word:  u64,
}

impl<'a, K, V, A: UnsignedNum> Cursor<'a, K, V, A> {
    fn new(map: &'a BlockMap<K, V, A>) -> Self {
        Cursor {
            map,
            rev: map.translator.reversed(),
            state: CursorSlot::FwdEnd,
            word: 0,
        }
    }
}

impl<'a, K: Ord, V, A: UnsignedNum> Cursor<'a, K, V, A> {
    ///reposition at the first key ≥ k
    pub fn seek(&mut self, k: &K) {
        match self.map.bsearch(k) {
            Ok(p) => self.land(p.0),
            Err(spot) => match self.map.key_bound(spot.0) {
                Some(q) => self.land(q),
                None => self.state = CursorSlot::FwdEnd,
            },
        }
    }

    ///reposition at the first key > k (a dup at k skipped)
    pub fn seek_after(&mut self, k: &K) {
        let q = match self.map.bsearch(k) {
            //past the dup in key order
            Ok(p) => {
                if self.rev { self.map.mask.prev_set(p.0) } else { self.map.mask.next_set(p.0 + 1) }
            }
            Err(spot) => self.map.key_bound(spot.0),
        };
        if let Some(q) = q { self.land(q) } else { self.state = CursorSlot::FwdEnd }
    }

    ///position at a live slot — the cache load (seeks, ghost resumes)
    fn land(&mut self, q: usize) {
        self.word = self.map.mask.word_at(q);
        self.state = CursorSlot::At(Pos(q));
    }

    ///yield the current pair, step forward (key-ascending). None at the
    ///walk's end
    #[inline(always)]
    pub fn next(&mut self) -> Option<(&'a K, &'a V)> {
        let p = match self.state {
            CursorSlot::FwdEnd => return None,
            CursorSlot::At(p) => p.0,
        };
        //Asc climbs positions; Desc descends them
        let q = if self.rev {
            self.map.mask.step_down(p, self.word)
        } else {
            self.map.mask.step_up(p, self.word)
        };
        self.settle(q);
        Some(unsafe { self.map.pair(p) })
    }

    ///step back (key-descending), yield the pair now current. None at
    ///the walk's start (the state stays at the first pair — next
    ///re-yields it); from the forward ghost, resumes at the last pair
    #[inline(always)]
    pub fn prev(&mut self) -> Option<(&'a K, &'a V)> {
        //the pair behind the current — the walk's last from FwdEnd
        let q = match self.state {
            CursorSlot::At(p) => {
                if self.rev {
                    self.map.mask.step_up(p.0, self.word)
                } else {
                    self.map.mask.step_down(p.0, self.word)
                }
            }
            CursorSlot::FwdEnd => {
                let q = (if self.rev { self.map.mask.next_set(0) }
                    else { self.map.mask.last_set() })?;
                self.land(q);
                return Some(unsafe { self.map.pair(q) });
            }
        }?;
        self.settle(Some(q));
        Some(unsafe { self.map.pair(q.0) })
    }

    ///land a step result; None is the walk's end (the cache goes stale)
    fn settle(&mut self, q: Option<(usize, u64)>) {
        match q {
            Some((q, w)) => {
                self.word = w;
                self.state = CursorSlot::At(Pos(q));
            }
            None => self.state = CursorSlot::FwdEnd,
        }
    }

    ///the pair next() would yield, without moving
    pub fn entry(&self) -> Option<(&'a K, &'a V)> {
        self.slot().map(|p| unsafe { self.map.pair(p) })
    }

    ///the current entry's name
    pub fn addr(&self) -> Option<A> {
        self.slot().map(|p| self.map.translator.p2a(Pos(p)))
    }

    ///At reads directly; FwdEnd has nothing ahead
    fn slot(&self) -> Option<usize> {
        match self.state {
            CursorSlot::At(p) => Some(p.0),
            CursorSlot::FwdEnd => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///apply translated deltas as name swaps, in the given order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64, u64)>, deltas: &Deltas<A>) {
        for (from, to) in deltas.iter() {
            for (a, _, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    fn insert_all(
        entries: impl IntoIterator<Item = (u64, u64)>,
    ) -> (BlockMap<u64, u64, u8>, Vec<(u8, u64, u64)>) {
        let mut map = BlockMap::new();
        let mut holder = Vec::new();
        let mut deltas = Deltas::new();
        for (k, v) in entries {
            match map.insert(k, v, &mut deltas) {
                Ok(addr) => {
                    apply(&mut holder, &deltas);
                    deltas.clear();
                    holder.push((addr, k, v));
                }
                Err((k, v)) => panic!("insert of ({k}, {v}) failed"),
            }
        }
        (map, holder)
    }

    fn check(map: &BlockMap<u64, u64, u8>, holder: &[(u8, u64, u64)], count: usize) {
        assert_eq!(map.len(), count);
        assert_eq!(holder.len(), count);
        let mut prev = None;
        for (k, _) in map.iter() {
            if let Some(p) = prev {
                assert!(*k > p, "order broken");
            }
            prev = Some(*k);
        }
        for (a, k, v) in holder {
            assert_eq!(map.get(*a), Some((k, v)), "holder name {a} broken");
        }
    }

    #[test]
    fn ascending() {
        let (m, h) = insert_all((0..200).map(|k| (k, k * 7)));
        check(&m, &h, 200);
    }

    ///front-biased stream: the reversal tier flips the block (see
    ///blockset's `descending`)
    #[test]
    fn descending() {
        let (m, h) = insert_all((0..200).rev().map(|k| (k, k + 1000)));
        check(&m, &h, 200);
    }

    #[test]
    fn stride() {
        let (m, h) = insert_all((0..200).map(|i| ((i * 37 + 11) % 211, i * 13)));
        check(&m, &h, 200);
    }

    ///mixed-op torture mirroring blockset's: clustered + descending-bait
    ///streams, random removals, every holder name verified after every op
    #[test]
    fn mixed_ops_torture() {
        let mut map = BlockMap::<u64, u64, u16>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for i in 0..600u64 {
            if holder.is_empty() || rnd() % 4 > 0 {
                let k = if i % 3 == 0 {
                    3000 - i * 5 + rnd() % 3
                } else {
                    (i / 40) * 700 + rnd() % 300
                };
                let v = k.wrapping_mul(3);
                match map.insert(k, v, &mut d) {
                    Ok(a) => {
                        apply(&mut holder, &d);
                        d.clear();
                        holder.push((a, k, v));
                    }
                    Err(_) => d.clear(),
                }
            } else {
                let j = (rnd() % holder.len() as u64) as usize;
                map.remove(holder[j].0);
                holder.swap_remove(j);
            }
            for &(a, k, v) in &holder {
                assert_eq!(map.get(a), Some((&k, &v)), "op {i}: name {a} broken");
            }
        }
        let keys: Vec<u64> = map.iter().map(|(k, _)| *k).collect();
        let asc = keys.windows(2).all(|w| w[0] < w[1]);
        let desc = keys.windows(2).all(|w| w[0] > w[1]);
        assert!(asc || desc, "order broken after the flip(s)");
        assert_eq!(keys.len(), holder.len());
    }

    ///appends stay dense; a mid insert into the full mask turns, the
    ///follow-up lands in a turn-minted hole (mirrors blockset's test)
    #[test]
    fn appends_dense_mid_insert_turns() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        for k in (0..100u64).map(|i| i * 2) {
            map.insert(k, k + 500, &mut d).unwrap();
            d.clear();
        }
        assert!(map.cap() < 128, "cap {} — turned instead of pushing back", map.cap());
        assert_eq!(map.cap() - map.len(), 0, "appends left holes");
        map.insert(101, 601, &mut d).unwrap();
        d.clear();
        assert_eq!(map.cap(), 200, "mid insert into full mask must turn");
        map.insert(103, 603, &mut d).unwrap();
        d.clear();
        assert_eq!(map.cap(), 200, "second mid insert must not turn");
        let keys: Vec<u64> = map.iter().map(|(k, _)| *k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(map.len(), 102);
        for (k, v) in map.iter() {
            assert_eq!(*v, k + 500, "values must ride the moves");
        }
    }

    ///fetch-distance banking (mirrors blockset's test): the 24 moved
    /// elements each emit ONE full-hop pair, then 3 banked inserts are
    /// no-ops
    #[test]
    fn gather_banks_holes_by_distance() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        for k in (0..=10u64).chain(60..=89).chain(200..=214) {
            let a = map.insert(k, k + 100, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k, k + 100));
        }
        //holes at pos 35..=50 — the 35..40 half is 24 from the spot of
        //key 30; the 41..50 half drops density below the 3/4 turn line
        for k in (84..=89u64).chain(200..=209) {
            map.remove(map.find(&k).unwrap());
            holder.retain(|(_, hk, _)| *hk != k);
        }
        let a = map.insert(30, 130, &mut d).unwrap();
        //identity translator: one hop-4 pair per moved element, farthest
        //first (see blockset's version)
        assert_eq!(
            d.iter().collect::<Vec<_>>(),
            (11u8..=34).rev().map(|q| (q, q + 4)).collect::<Vec<_>>(),
            "24 moved elements, one hop-4 pair each"
        );
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 30, 130));
        assert_eq!(map.cap(), 56, "gather must not turn");
        for k in [31, 32, 33] {
            let a = map.insert(k, k + 100, &mut d).unwrap();
            assert_eq!(d.iter().count(), 0, "banked hole should absorb insert {k}");
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k, k + 100));
        }
        check(&map, &holder, 44);
    }

    ///front insert below cap: plain slide, both vecs move — 5 takes name 0
    #[test]
    fn front_insert_renames_min() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        map.insert(10, 110, &mut d).unwrap();
        d.clear();
        map.insert(20, 120, &mut d).unwrap();
        d.clear();
        let a = map.insert(5, 105, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(1, 2), (0, 1)]);
        assert_eq!(map.get(0), Some((&5, &105)));
        assert_eq!(map.get(1), Some((&10, &110)));
        assert_eq!(map.get(2), Some((&20, &120)));
    }

    ///tiny dense block, front insert: the ladder opens a slot at the back
    ///and slides the run into it — no rotation
    #[test]
    fn front_insert_back_opens() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        for k in [10, 20, 30, 40] {
            map.insert(k, k + 1, &mut d).unwrap();
            d.clear();
        }
        let a = map.insert(5, 6, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(3, 4), (2, 3), (1, 2), (0, 1)]);
        assert_eq!(map.get(0), Some((&5, &6)));
        assert_eq!(map.get(1), Some((&10, &11)));
        assert_eq!(map.get(4), Some((&40, &41)));
        assert_eq!(map.cap(), 5);
    }

    #[test]
    fn duplicates_rejected() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        map.insert(1, 11, &mut d).unwrap();
        assert_eq!(map.insert(1, 99, &mut d), Err((1, 99)));
        assert_eq!(map.len(), 1);
        assert_eq!(map.value(&1), Some(&11), "rejected insert must not overwrite");
    }

    ///insert leaves a pending (no-op) event — the next insert must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        map.insert(1, 11, &mut d).unwrap();
        map.insert(2, 12, &mut d).unwrap();
        map.insert(3, 13, &mut d).unwrap();
    }

    #[test]
    fn remove_and_reinsert() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        for k in 0..50u64 {
            map.insert(k, k * 2, &mut d).unwrap();
            d.clear();
        }
        let a = map.find(&25).unwrap();
        assert_eq!(map.remove(a), Some((25, 50)));
        assert_eq!(map.remove(a), None);
        assert_eq!(map.remove_key(&25), None);
        map.insert(25, 50, &mut d).unwrap();
        let mut keys: Vec<u64> = map.iter().map(|(k, _)| *k).collect();
        keys.sort();
        assert_eq!(keys, (0..50).collect::<Vec<_>>());
    }

    ///value/value_mut by key, and the values survive a reversal + turn
    #[test]
    fn value_lookup_and_mut() {
        let mut map = BlockMap::<u64, u64, u8>::new();
        let mut d = Deltas::new();
        for k in (0..60u64).rev() {
            map.insert(k, k + 1, &mut d).unwrap();
            d.clear();
        }
        assert!(map.iter().all(|(k, v)| *v == k + 1));
        *map.value_mut(&30).unwrap() = 999;
        assert_eq!(map.value(&30), Some(&999));
        assert_eq!(map.value(&100), None);
    }

    ///the full cursor battery (see blockset::assert_walks): both seeks ×
    ///both walk directions at every boundary, peek + addr, ghost
    ///round-trips. live = sorted live (key, value) pairs
    fn assert_walks<A: UnsignedNum>(map: &BlockMap<u64, u64, A>, live: &[(u64, u64)]) {
        let max = live.last().unwrap().0 + 1;
        for k in 0..=max {
            let mut c = map.seek(&k);
            let peek = live.iter().find(|(x, _)| *x >= k).copied();
            assert_eq!(c.entry().map(|(k, v)| (*k, *v)), peek, "peek {k}");
            if let (Some((x, _)), Some(a)) = (peek, c.addr()) {
                assert_eq!(map.find(&x), Some(a), "addr {k}");
            }
            let expect: Vec<(u64, u64)> = live.iter().copied().filter(|(x, _)| *x >= k).collect();
            let got: Vec<(u64, u64)> =
                std::iter::from_fn(|| c.next().map(|(k, v)| (*k, *v))).collect();
            assert_eq!(got, expect, "seek({k}) forward");
            let mut c = map.seek(&k);
            let expect: Vec<(u64, u64)> =
                live.iter().rev().copied().filter(|(x, _)| *x < k).collect();
            let got: Vec<(u64, u64)> =
                std::iter::from_fn(|| c.prev().map(|(k, v)| (*k, *v))).collect();
            assert_eq!(got, expect, "seek({k}) back");
            let mut c = map.seek_after(&k);
            let expect: Vec<(u64, u64)> = live.iter().copied().filter(|(x, _)| *x > k).collect();
            let got: Vec<(u64, u64)> =
                std::iter::from_fn(|| c.next().map(|(k, v)| (*k, *v))).collect();
            assert_eq!(got, expect, "seek_after({k}) forward");
            let mut c = map.seek_after(&k);
            let expect: Vec<(u64, u64)> =
                live.iter().rev().copied().filter(|(x, _)| *x <= k).collect();
            let got: Vec<(u64, u64)> =
                std::iter::from_fn(|| c.prev().map(|(k, v)| (*k, *v))).collect();
            assert_eq!(got, expect, "seek_after({k}) back");
        }
        //round-trip across the FwdEnd ghost; exhaustion is stable, both
        //sides resume forward correctly
        let mut c = map.seek(&live[0].0);
        for kv in live {
            assert_eq!(c.next(), Some((&kv.0, &kv.1)), "roundtrip forward");
        }
        assert_eq!(c.next(), None);
        assert_eq!(c.next(), None, "FwdEnd must be stable");
        for kv in live.iter().rev() {
            assert_eq!(c.prev(), Some((&kv.0, &kv.1)), "roundtrip back");
        }
        assert_eq!(c.prev(), None);
        assert_eq!(c.prev(), None, "start must be stable");
        assert_eq!(c.next(), Some((&live[0].0, &live[0].1)), "start resumes forward");
        let mut c = map.seek(&live[0].0);
        assert_eq!(c.prev(), None);
        assert_eq!(c.next(), Some((&live[0].0, &live[0].1)));
        assert_eq!(c.next(), Some((&live[1].0, &live[1].1)));
    }

    ///holed mask, forward block: cursor walks match the sorted model
    #[test]
    fn cursor_matches_sorted_model() {
        let mut map = BlockMap::<u64, u64, u16>::new();
        let mut d = Deltas::new();
        //37 coprime 67 ⇒ distinct keys, 7 gaps for hole spots (sized for miri)
        for i in 0..60u64 {
            let k = (i * 37 + 11) % 67;
            map.insert(k, k * 7 + 3, &mut d).unwrap();
            d.clear();
        }
        for k in [11u64, 48, 18, 59, 66] {
            if let Some(a) = map.find(&k) {
                map.remove(a);
            }
        }
        let mut live: Vec<(u64, u64)> = map.iter().map(|(k, v)| (*k, *v)).collect();
        live.sort_unstable();
        assert_walks(&map, &live);
    }

    ///flipped block: identical key-order semantics, values ride (see
    ///blockset's `cursor_flipped_block`)
    #[test]
    fn cursor_flipped_block() {
        let (mut map, _) = insert_all((0..60u64).rev().map(|k| (k, k * 7)));
        let removed = [3u64, 17, 42];
        for k in removed {
            map.remove_key(&k);
        }
        let live: Vec<(u64, u64)> =
            (0..60u64).filter(|k| !removed.contains(k)).map(|k| (k, k * 7)).collect();
        assert_walks(&map, &live);
    }

    #[test]
    fn cursor_empty_and_single() {
        let map = BlockMap::<u64, u64, u8>::new();
        let mut c = map.seek(&5);
        assert_eq!(c.entry(), None);
        assert_eq!(c.addr(), None);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), None);
        let mut map = map;
        let mut d = Deltas::new();
        map.insert(7, 70, &mut d).unwrap();
        d.clear();
        let mut c = map.seek(&5);
        assert_eq!(c.next(), Some((&7, &70)));
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some((&7, &70)));
        assert_eq!(c.prev(), None);
        assert_eq!(c.next(), Some((&7, &70)));
        let mut c = map.seek_after(&7);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some((&7, &70)));
    }
}
