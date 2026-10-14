//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order (hole-adjacent mover
//!first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme is always a physical tail
//!push_back. the translator carries the direction (see translator.rs).

use std::cmp::Ordering;
use std::mem::MaybeUninit;

use super::store::NoneSlide;
use super::translator::Translator;
use crate::index::*;
use crate::mask::{Mask, SetBits, SetBitsRev};

///one mutation event, compact. `Empty` = nothing pending (initial state;
///`clear` restores it — a no-op slide is still pending, `iter` just yields
///nothing). the common contiguous run slide (the hole moves `from → to`;
///every element strictly between shifts one step toward `from`). `Gather`:
///a fetch-distance gather's slide chain, nearest hole first, each slide
///in post-prior-slides coordinates — identical to its pre-read position,
///since a hole's slot is never touched before its own slide; a two-sided
///gather is two chains' worth, side told by `from > to`. `Pairs` covers
///rare non-run shapes (reversal, bulk).
enum Delta {
    Empty,
    NoneSlide {
        from: Pos,
        to:   Pos,
    },
    Gather(Vec<NoneSlide>),
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}

///expanded `(from, to)` pos pairs of one event, swap-cascade order
///(hole-adjacent mover first). `Run`: `p` cursor, `step` (−1 = run shifts
///up), `remaining` emissions; each pair is `(p, p − step)`. `Gather` chains
///the slides' runs in order (each slide's coordinates are post-prior-slides).
enum DeltaPairs<'a> {
    Empty,
    Run { p: usize, step: i64, remaining: usize },
    Gather {
        slides:    std::slice::Iter<'a, NoneSlide>,
        p:         usize,
        step:      i64,
        remaining: usize,
    },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}

///swap-cascade run state of one hole slide `from → to`
fn run_of(from: usize, to: usize) -> (usize, i64, usize) {
    if from > to {
        (from - 1, -1, from - to)
    } else {
        (from + 1, 1, to - from)
    }
}

impl Delta {
    fn pairs(&self) -> DeltaPairs<'_> {
        match self {
            Delta::Empty => DeltaPairs::Empty,
            Delta::NoneSlide { from, to } => {
                let (p, step, remaining) = run_of(from.0, to.0);
                DeltaPairs::Run { p, step, remaining }
            }
            Delta::Gather(v) => DeltaPairs::Gather {
                slides:    v.iter(),
                p:         0,
                step:      0,
                remaining: 0,
            },
            Delta::Pairs(v) => DeltaPairs::Pairs(v.iter()),
        }
    }
}

impl Iterator for DeltaPairs<'_> {
    type Item = (Pos, Pos);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            DeltaPairs::Empty => None,
            DeltaPairs::Run { p, step, remaining } => {
                if *remaining == 0 {
                    return None;
                }
                *remaining -= 1;
                let q = *p;
                *p = (q as i64 + *step) as usize;
                Some((Pos(q), Pos((q as i64 - *step) as usize)))
            }
            DeltaPairs::Gather { slides, p, step, remaining } => loop {
                if *remaining == 0 {
                    let ns = slides.next()?;
                    (*p, *step, *remaining) = run_of(ns.from.0, ns.to.0);
                }
                *remaining -= 1;
                let q = *p;
                *p = (q as i64 + *step) as usize;
                return Some((Pos(q), Pos((q as i64 - *step) as usize)));
            },
            DeltaPairs::Pairs(it) => it.next().copied(),
        }
    }
}

///slot moves of one mutation, compact event + translator snapshot at
///mutation time. `iter` expands to (from_addr, to_addr) name swaps
///applicable in order. per-call: `insert` panics while the previous call's
///deltas are pending — apply, then `clear`.
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}

impl<A: UnsignedNum> Deltas<A> {
    pub fn new() -> Self {
        Deltas { event: Delta::Empty, translator: Translator::new() }
    }

    ///(from_addr, to_addr) name swaps, applicable in order
    pub fn iter(&self) -> impl Iterator<Item = (A, A)> + '_ {
        self.event.pairs().map(|(f, t)| (self.translator.p2a(f), self.translator.p2a(t)))
    }

    ///pending until `clear` — the caller contract
    fn is_pending(&self) -> bool {
        !matches!(self.event, Delta::Empty)
    }

    ///reset to empty — call after applying
    pub fn clear(&mut self) {
        self.event = Delta::Empty;
        self.translator = Translator::new()
    }
}

///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
const FREE_STRIDE: usize = 8;

///mask covers data.len() slots; bit ⇔ slot initialized.
pub struct BlockSet<K, A> {
    translator: Translator<A>,
    data:       Vec<MaybeUninit<K>>,
    mask:       Mask,
    len:        usize,
    ///running sum of placed slot positions — the mean-insert-position
    ///signal for the reversal tier
    #[allow(dead_code)]
    pos_sum:    u64,
}

impl<K, A> Drop for BlockSet<K, A> {
    fn drop(&mut self) {
        for p in self.mask.iter_set() {
            //mask ⇔ init invariant
            unsafe { self.data[p].assume_init_drop() };
        }
    }
}

impl<K: Ord, A: UnsignedNum> BlockSet<K, A> {
    pub fn new() -> Self {
        BlockSet {
            translator: Translator::new(),
            data:      Vec::new(),
            mask:      Mask::new(),
            len:       0,
            pos_sum:   0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn cap(&self) -> usize {
        self.data.len()
    }

    pub fn max_capacity() -> usize {
        A::MAX.as_usize().saturating_add(1)
    }

    ///None if addr names no live element (never-minted names can't alias a
    ///live slot — a2p is injective; STALE pre-delta names can, see
    ///subtle_bugs §17)
    pub fn get(&self, addr: A) -> Option<&K> {
        let p = self.translator.a2p(addr);
        (p.0 < self.data.len() && self.mask.get(p.0))
            .then(|| unsafe { self.data[p.0].assume_init_ref() })
    }

    #[inline]
    pub fn find(&self, k: &K) -> Option<A> {
        match self.position(k) {
            Ok(p) => Some(self.translator.p2a(p)),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    ///first→last over live keys. the direction is picked once here, never
    ///per element — Asc walks positions up, Desc down (key order ascends
    ///against pos order)
    pub fn iter(&self) -> Iter<'_, K, A> {
        if self.translator.reversed() {
            Iter::Desc { bits: self.mask.iter_set_rev(), set: self }
        } else {
            Iter::Asc { bits: self.mask.iter_set(), set: self }
        }
    }

    ///deltas must be empty (applied + cleared) — panics if pending. Err
    ///returns k (dup, or out of names).
    pub fn insert(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.data.is_empty() {
            return Ok(self.insert_first(k));
        }
        if self.len >= Self::max_capacity() {
            return Err(k);
        }
        let Some(spot) = self.spot(&k) else {
            return Err(k);
        };
        //sequential at the allocation edge: extend instead of sliding
        //k belongs at the back (spot past the whole array): extend the
        //allocation — the cheapest space is the edge
        if spot.0 == self.data.len() && self.data.len() < Self::max_capacity() {
            let a = self.push_back(k, spot);
            self.maybe_reverse();
            return Ok(a);
        }
        let p = self.open(&k, spot, deltas);
        let a = self.place(k, p);
        self.maybe_reverse();
        Ok(a)
    }

    ///in-place reversal: the array mirrors (pos 0 = max), names follow the
    ///slots — the translator absorbs it, no deltas (a pending event stays
    ///valid: it speaks names, and names never change). pos_sum reflects
    ///around len: the historical insert positions read in the new
    ///orientation
    fn reverse(&mut self) {
        self.data.reverse();
        self.mask.reverse();
        self.translator = self.translator.reverse(self.data.len());
        let len = self.len as u64;
        self.pos_sum = len.wrapping_mul(len).wrapping_sub(self.pos_sum);
    }

    ///front-biased insert stream ⇒ flip, so the stream's next inserts
    ///land at the tail. the signal must be committed — mean insert position
    ///below len/8 past a 4·stride warmup: at len/4 a uniform stream's small
    ///early window flips the block spuriously (4 flips on pure random,
    ///bench_notes), and a real front stream's mean sits ≈ 0 either way
    fn maybe_reverse(&mut self) {
        if self.len > 4 * FREE_STRIDE {
            let len = self.len as u64;
            if self.pos_sum.wrapping_mul(8) < len.wrapping_mul(len) {
                self.reverse();
            }
        }
    }

    ///insertion spot, or None on dup. beyond the tail extreme the spot is
    ///known without bsearch (max Asc, min Desc — the end a sequential
    ///insert extends)
    fn spot(&self, k: &K) -> Option<Pos> {
        if self.len > 0 {
            let tail = self.key_tail();
            let beyond = if self.translator.reversed() { k < tail } else { tail < k };
            if beyond {
                return Some(Pos(self.mask.last_set().unwrap() + 1));
            }
        }
        self.position(k).err()
    }

    ///vec-like first allocation: 4 slots, no translator turn until it fills
    fn insert_first(&mut self, k: K) -> A {
        self.data = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.mask = Mask::with_bits(4);
        self.place(k, Pos(0))
    }

    ///extend the allocation and place k at the edge — dense; banked holes
    ///come from `gather_slots`' fetch, not minted here. caller guarantees
    ///spot == cap < max_capacity.
    fn push_back(&mut self, k: K, spot: Pos) -> A {
        self.push_slot();
        self.place(k, spot)
    }

    ///open a slot at/near `spot`: a preemptive turn past 7/8 occupancy when
    ///the insert sits mid-block (at one hole per FREE_STRIDE expected, past
    ///7/8 fetches blow past the stride — one O(n) turn beats the long
    ///gathers); then the fetch gather (nearest hole sets the distance,
    ///distance sets the count — banked holes ride the slide). mask full ⇒
    ///one tail push when the edge is within FREE_STRIDE, else a turn;
    ///refused ⇒ plain tail slot (room always exists: mask full + len <
    ///name ceiling).
    fn open(&mut self, k: &K, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let mut spot = spot;
        if self.len.wrapping_mul(8) > self.data.len().wrapping_mul(7)
            && self.data.len() - spot.0 > FREE_STRIDE
            && self.grow_rotate()
        {
            spot = self.position(k).err().expect("no dup post-grow");
        }
        if let Some(p) = self.gather_slots(spot, deltas) {
            return p;
        }
        if self.data.len() - spot.0 <= FREE_STRIDE && self.data.len() < Self::max_capacity() {
            self.push_slot();
        } else if self.grow_rotate() {
            spot = self.position(k).err().expect("no dup post-grow");
        } else {
            self.push_slot();
        }
        self.gather_slots(spot, deltas).expect("mask had a hole minted")
    }

    ///write k at an open slot, mark live, mint its name; the slot position
    ///joins the running insert-position sum
    fn place(&mut self, k: K, p: Pos) -> A {
        self.data[p.0] = MaybeUninit::new(k);
        self.mask.set(p.0, true);
        self.len += 1;
        self.pos_sum = self.pos_sum.wrapping_add(p.0 as u64);
        self.translator.p2a(p)
    }

    pub fn remove(&mut self, addr: A) -> Option<K> {
        self.remove_pos(self.translator.a2p(addr))
    }

    ///None if addr names no live element (stale-name hazard: get/remove_pos)
    fn remove_pos(&mut self, pos: Pos) -> Option<K> {
        if pos.0 < self.data.len() && self.mask.get(pos.0) {
            self.mask.set(pos.0, false);
            self.len -= 1;
            //mask ⇔ init invariant
            return Some(unsafe { self.data[pos.0].assume_init_read() });
        }
        None
    }

    ///bsearch over live slots. Ok(p) = duplicate found at p; Err(spot) = the
    ///insertion spot: first live pos past the run that will precede k.
    ///holes are skipped forward — sortedness makes everything live before
    ///the probe compare known. the probe/k operand order is the block's
    ///direction, fixed per arm so the hot loop carries no branch
    fn position(&self, k: &K) -> Result<Pos, Pos> {
        if self.translator.reversed() {
            self.bsearch(k, |probe, k| k.cmp(probe))
        } else {
            self.bsearch(k, |probe, k| probe.cmp(k))
        }
    }

    fn bsearch(&self, k: &K, locate: impl Fn(&K, &K) -> Ordering) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (0, self.data.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => match locate(self.key(p), k) {
                    Ordering::Less => lo = p + 1,
                    Ordering::Greater => hi = p,
                    Ordering::Equal => return Ok(Pos(p)),
                },
            }
        }
        Err(Pos(lo))
    }

    fn next_live(&self, from: usize, bound: usize) -> Option<usize> {
        self.mask.next_set(from).filter(|p| *p < bound)
    }

    ///mask ⇔ init invariant upholds this read
    fn key(&self, p: usize) -> &K {
        unsafe { self.data[p].assume_init_ref() }
    }

    ///key at the physical tail (max Asc, min Desc); panics if len == 0
    fn key_tail(&self) -> &K {
        self.key(self.mask.last_set().expect("live"))
    }

    ///fetch-distance gather: the nearest hole's distance from `spot` sets
    ///the fetch — one hole per FREE_STRIDE traveled (the further the look,
    ///the more Nones come back, banked beside k for the cluster's next
    ///inserts). holes all from the nearest hole's side. k's slot: hole
    ///right ⇒ `spot` (run shifts up), left ⇒ `spot − 1` (run shifts
    ///down). fails clean on a hole-free mask.
    fn gather_slots(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Option<Pos> {
        //k's slot already open (the bsearch converges on holes for
        //mid-gap inserts) — no fetch, no slide. nearest_clear excludes
        //`spot` by contract, so this is checked first
        if spot.0 < self.cap() && !self.mask.get(spot.0) {
            deltas.translator = self.translator;
            deltas.event = Delta::NoneSlide { from: spot, to: spot };
            return Some(spot);
        }
        let hole = self.mask.nearest_clear(spot.0)?;
        deltas.translator = self.translator;
        let right = hole > spot.0;
        let open = if right { spot.0 } else { spot.0 - 1 };
        //the common case: hole within a stride — one plain slide, no fetch
        if hole.abs_diff(spot.0) < FREE_STRIDE {
            self.slide_hole(hole, open);
            deltas.event = Delta::NoneSlide { from: Pos(hole), to: Pos(open) };
            return Some(Pos(open));
        }
        let holes = self.fetch_holes(spot, right, 1 + hole.abs_diff(spot.0) / FREE_STRIDE);
        self.compact_gather(spot, open, right, &holes);
        deltas.event = Delta::Gather(
            holes
                .iter()
                .enumerate()
                .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                .collect(),
        );
        Some(Pos(open))
    }

    ///one plain slide: hole `from` → `to`; the run between is fully live
    ///(nearest hole), so the mask update is two bits. the bitwise duplicate
    ///ptr::copy leaves in the source range is uninit by mask, never
    ///assume_init'd (mask ⇔ init invariant).
    fn slide_hole(&mut self, from: usize, to: usize) {
        unsafe {
            let ptr = self.data.as_mut_ptr();
            if from > to {
                std::ptr::copy(ptr.add(to), ptr.add(to + 1), from - to);
            } else {
                std::ptr::copy(ptr.add(from + 1), ptr.add(from), to - from);
            }
        }
        self.mask.set(from, true);
        self.mask.set(to, false);
    }

    ///the `want` nearest holes on one side of `spot`, nearest-first
    ///(right: ascending, left: descending) — pre-read positions, consumed
    ///by `compact_gather` without further scans
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

    ///gather compaction, one pass: each member crosses exactly once —
    ///segment j (between consecutive holes) shifts by its crossing count,
    ///far segments copy first so no live source is read after being
    ///overwritten. mask: the holes fill, the vacated target range
    ///(`[open, open+w)` right, `[open−w+1, open]` left) clears; sets first
    ///— a hole compacted onto its own old position ends clear.
    fn compact_gather(&mut self, spot: Pos, open: usize, right: bool, holes: &[usize]) {
        let w = holes.len();
        unsafe {
            let ptr = self.data.as_mut_ptr();
            if right {
                //holes ascend; segment 0 spans [spot, h_0), the run k shifts
                for j in (0..w).rev() {
                    let (lo, len) = if j == 0 {
                        (spot.0, holes[0] - spot.0)
                    } else {
                        (holes[j - 1] + 1, holes[j] - holes[j - 1] - 1)
                    };
                    let shift = w - j;
                    std::ptr::copy(ptr.add(lo), ptr.add(lo + shift), len);
                }
            } else {
                //holes descend; the top segment ends at spot − 1 (the
                //element at spot never moves — k lands below it)
                for k in (0..w).rev() {
                    let (lo, len) = if k == 0 {
                        (holes[0] + 1, spot.0 - 1 - holes[0])
                    } else {
                        (holes[k] + 1, holes[k - 1] - holes[k] - 1)
                    };
                    let shift = w - k;
                    std::ptr::copy(ptr.add(lo), ptr.add(lo - shift), len);
                }
            }
        }
        for &h in holes {
            self.mask.set(h, true);
        }
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

    ///rotation turn (in-place): cap doubles; every live element relocates
    ///to `a2p_new(name)` — names preserved by construction, the map
    ///monotone (order holds), each target ≥ its source (the descending
    ///pass writes every target before its source is read). refused past
    ///the name-space midpoint (doubled positions would exceed the address
    ///space) or at the rotation budget. any offset, either direction: the
    ///band (upper-half names land odd) is rotate_mask's job.
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.data.len();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        if old_cap > A::MIDPOINT.as_usize()
            || self.translator.rotation() >= A::BIT_WIDTH as u32
        {
            return false;
        }
        let new_tr = self.translator.rotate();
        self.data.reserve(old_cap);
        self.data.extend((old_cap..new_cap).map(|_| MaybeUninit::uninit()));
        for p in (0..old_cap).rev() {
            if self.mask.get(p) {
                let q = new_tr.a2p(self.translator.p2a(Pos(p)));
                debug_assert!(q.0 >= p, "turn must not move an element down");
                self.data[q.0] = std::mem::replace(&mut self.data[p], MaybeUninit::uninit());
            }
        }
        self.mask.rotate_mask(
            self.translator.offset().as_usize(),
            A::MIDPOINT.as_usize(),
            self.translator.reversed(),
        );
        self.translator = new_tr;
        //the pos-space doubled — the insert-position sum tracks it
        self.pos_sum = self.pos_sum.wrapping_mul(2);
        true
    }

    ///extend by one uninit slot (mask clear) — the Vec's own doubling
    ///carries reallocation. panics at the name-space ceiling.
    fn push_slot(&mut self) {
        assert!(self.data.len() < Self::max_capacity(), "name space exhausted");
        self.data.push(MaybeUninit::uninit());
        self.mask.resize(self.data.len());
    }

    pub fn from_vec(v: Vec<K>) -> Self {
        let _ = v;
        todo!()
    }
}

///`BlockSet::iter`'s differential — one enum dispatch per element, the
///per-direction bodies specialized (word-stepping mask iteration)
pub enum Iter<'a, K, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}

impl<'a, K: Ord, A: UnsignedNum> Iterator for Iter<'a, K, A> {
    type Item = &'a K;

    #[inline]
    fn next(&mut self) -> Option<&'a K> {
        match self {
            //mask ⇔ init invariant upholds the read
            Iter::Asc { bits, set } => bits.next().map(|p| set.key(p)),
            Iter::Desc { bits, set } => bits.next().map(|p| set.key(p)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///apply translated deltas as name swaps, in the given order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &Deltas<A>) {
        for (from, to) in deltas.iter() {
            for (a, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>) {
        let mut set = BlockSet::new();
        let mut holder = Vec::new();
        let mut deltas = Deltas::new();
        for k in keys {
            match set.insert(k, &mut deltas) {
                Ok(addr) => {
                    apply(&mut holder, &deltas);
                    deltas.clear();
                    holder.push((addr, k));
                }
                Err(k) => panic!("insert of {k} failed"),
            }
        }
        (set, holder)
    }

    fn check(set: &BlockSet<u64, u8>, holder: &[(u8, u64)], count: usize) {
        assert_eq!(set.len(), count);
        assert_eq!(holder.len(), count);
        let mut prev = None;
        for k in set.iter() {
            if let Some(p) = prev {
                assert!(*k > p, "order broken");
            }
            prev = Some(*k);
        }
        for (a, k) in holder {
            assert_eq!(set.get(*a), Some(k), "holder name {a} -> key {k} broken");
        }
    }

    #[test]
    fn ascending() {
        let (s, h) = insert_all(0..200);
        check(&s, &h, 200);
    }

    ///front-biased stream: the mean-insert trigger flips the block past the
    ///warmup — the remaining inserts are tail pushes (they never reach the
    ///turn rung), names ride the flip, the holder mirror validates them
    #[test]
    fn descending() {
        let (s, h) = insert_all((0..200).rev());
        check(&s, &h, 200);
        assert!(s.translator.reversed(), "front bias must flip the block");
    }

    #[test]
    fn stride() {
        let (s, h) = insert_all((0..200).map(|i| (i * 37 + 11) % 211));
        check(&s, &h, 200);
    }

    ///mixed-op torture: clustered streams with interleaved descending bait
    ///(the flip) and random removals — gathers bank, turns fire under
    ///either direction, and every holder name is verified after every op
    #[test]
    fn mixed_ops_torture() {
        let mut set = BlockSet::<u64, u16>::new();
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
                //descending stretches bait the flip; clusters bait the gather
                let k = if i % 3 == 0 {
                    3000 - i * 5 + rnd() % 3
                } else {
                    (i / 40) * 700 + rnd() % 300
                };
                match set.insert(k, &mut d) {
                    Ok(a) => {
                        apply(&mut holder, &d);
                        d.clear();
                        holder.push((a, k));
                    }
                    Err(_) => d.clear(),
                }
            } else {
                let j = (rnd() % holder.len() as u64) as usize;
                set.remove(holder[j].0);
                holder.swap_remove(j);
            }
            for &(a, k) in &holder {
                assert_eq!(set.get(a), Some(&k), "op {i}: name {a} broken");
            }
        }
        let keys: Vec<u64> = set.iter().copied().collect();
        let asc = keys.windows(2).all(|w| w[0] < w[1]);
        let desc = keys.windows(2).all(|w| w[0] > w[1]);
        assert!(asc || desc, "order broken after the flip(s)");
        assert_eq!(keys.len(), holder.len());
    }

    ///appends stay dense — cap tracks n, no cadence holes; a mid insert
    ///into the full mask turns, and the follow-up mid insert lands in a
    ///turn-minted hole — no second turn
    #[test]
    fn appends_dense_mid_insert_turns() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in (0..100u64).map(|i| i * 2) {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        assert!(set.cap() < 128, "cap {} — turned instead of pushing back", set.cap());
        assert_eq!(set.cap() - set.len(), 0, "appends left holes");
        set.insert(101, &mut d).unwrap();
        d.clear();
        assert_eq!(set.cap(), 200, "mid insert into full mask must turn");
        set.insert(103, &mut d).unwrap();
        d.clear();
        assert_eq!(set.cap(), 200, "second mid insert must not turn");
        let keys: Vec<u64> = set.iter().copied().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(set.len(), 102);
    }

    ///fetch-distance banking: holes 24 slots from the spot fetch 4 (one per
    ///FREE_STRIDE traveled) and land beside k — the next 3 inserts are
    ///no-ops. the holder rides the 96-pair chain (4 slides × 24-slot runs)
    #[test]
    fn gather_banks_holes_by_distance() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        for k in (0..=10u64).chain(60..=89) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        //the only holes: pos 35..=40, 24 from the spot of key 30
        for k in 84..=89 {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        let a = set.insert(30, &mut d).unwrap();
        assert_eq!(d.iter().count(), 96, "4 slides × 24-slot runs");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 30));
        assert_eq!(set.cap(), 41, "gather must not turn");
        for k in [31, 32, 33] {
            let a = set.insert(k, &mut d).unwrap();
            assert_eq!(d.iter().count(), 0, "banked hole should absorb insert {k}");
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        check(&set, &holder, 39);
    }

    ///front insert below cap: plain slide, name transfer without any turn —
    ///the run 20,10 cascades right (hole-adjacent first), 5 takes name 0
    #[test]
    fn front_insert_renames_min() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(10, &mut d).unwrap();
        d.clear();
        set.insert(20, &mut d).unwrap();
        d.clear();
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(2), Some(&20));
    }

    ///tiny dense block, front insert: the tail run is within budget, so
    ///the ladder opens a slot at the back and slides the run into it —
    ///hole-adjacent cascade, no rotation (small caps never turn)
    #[test]
    fn front_insert_back_opens() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in [10, 20, 30, 40] {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(3, 4), (2, 3), (1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(4), Some(&40));
        assert_eq!(set.cap(), 5);
    }

    #[test]
    fn duplicates_rejected() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(1, &mut d).unwrap();
        assert!(set.insert(1, &mut d).is_err());
        assert_eq!(set.len(), 1);
    }

    ///the compact NoneSlide event expands to the same cascade the old
    ///per-element pairs produced, both directions
    #[test]
    fn none_slide_expands_to_cascade() {
        let expand = |from: usize, to: usize| -> Vec<(Pos, Pos)> {
            Delta::NoneSlide { from: Pos(from), to: Pos(to) }.pairs().collect()
        };
        assert_eq!(
            expand(10, 5),
            vec![
                (Pos(9), Pos(10)),
                (Pos(8), Pos(9)),
                (Pos(7), Pos(8)),
                (Pos(6), Pos(7)),
                (Pos(5), Pos(6))
            ],
            "run shifts up"
        );
        assert_eq!(
            expand(5, 9),
            vec![(Pos(6), Pos(5)), (Pos(7), Pos(6)), (Pos(8), Pos(7)), (Pos(9), Pos(8))],
            "run shifts down"
        );
        assert_eq!(expand(7, 7), Vec::<(Pos, Pos)>::new(), "from == to = no-op");
    }

    ///insert(2) leaves a pending (no-op) event — insert(3) must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(1, &mut d).unwrap();
        set.insert(2, &mut d).unwrap();
        set.insert(3, &mut d).unwrap();
    }

    ///the turn map (`a2p_new ∘ p2a_old`) is name-stable, order-preserving
    ///and in-bounds for every reachable translator — including the offsets
    ///grow_rotate currently refuses
    #[test]
    fn rotate_map_preserves_names_and_order() {
        for r in 0..u8::BIT_WIDTH as u32 {
            for o in [0u8, 1, 2, 63, 64, 127, 128, 200, 255] {
                let old = Translator::<u8>::with_params(r, o);
                let new = old.rotate();
                let mut prev = None;
                for p in 0..100usize {
                    let q = new.a2p(old.p2a(Pos(p)));
                    assert_eq!(new.p2a(q), old.p2a(Pos(p)), "names r={r} o={o} p={p}");
                    assert!(q.0 < 200, "bounds r={r} o={o} p={p}");
                    if let Some(pq) = prev {
                        assert!(q.0 > pq, "order r={r} o={o} p={p}");
                    }
                    prev = Some(q.0);
                }
            }
        }
    }

    #[test]
    fn remove_and_reinsert() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in 0..50u64 {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        let a = set.find(&25).unwrap();
        assert_eq!(set.remove(a), Some(25));
        assert_eq!(set.remove(a), None);
        set.insert(25, &mut d).unwrap();
        let mut keys: Vec<u64> = set.iter().copied().collect();
        keys.sort();
        assert_eq!(keys, (0..50).collect::<Vec<_>>());
    }
}


