//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order (hole-adjacent mover
//!first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.
//!order: pos order == key order — normal (pos 0 = min) or reversed (pos 0
//!= max, Prepend bias), so a sequential insert at the key extreme is always
//!a physical tail push_back.

use std::cmp::Ordering;
use std::mem::MaybeUninit;

use crate::index::*;
use crate::mask::Mask;

///workload vocabulary, fixed for the block's life. the space ladder is
///unified (edge pushes, budgeted probes, dense turns), so only key
///direction differs: Prepend stores the block reversed (pos 0 = max), so
///a sequential prepend is a tail push_back, never a slide. Append and
///Insert are equivalent today; the heuristic tier may re-diverge them
///(e.g. per-workload turn thresholds).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InsertBias {
    Append,
    Prepend,
    Insert,
}

///key order along pos — Desc stores keys descending (pos 0 = max), so a
///sequential prepend is a tail push_back, never a slide
enum Direction {
    Asc,
    Desc,
}

impl Direction {
    ///k beyond the tail extreme (the end a sequential insert extends —
    ///max Asc, min Desc)?
    fn beyond_tail<K: Ord>(&self, k: &K, tail: &K) -> bool {
        match self {
            Direction::Asc => tail < k,
            Direction::Desc => k < tail,
        }
    }

    ///k vs the probe in scan order — Desc flips the operands, the whole of
    ///the reversal. Less: the spot lies past p; Greater: before p.
    fn locate<K: Ord>(&self, k: &K, probe: &K) -> Ordering {
        match self {
            Direction::Asc => probe.cmp(k),
            Direction::Desc => k.cmp(probe),
        }
    }
}

///`a2p(a) = (a rol rotation) + offset`, `p2a` the exact inverse (both wrap in
///A). rotation turn = growth: every live pos maps to `2·pos − offset` mod A.
///offset turn = window move (heuristic tier, unimplemented).
#[derive(Clone, Copy)]
struct Translator<A> {
    rotation: u32,
    offset:   A,
}

impl<A: UnsignedNum> Translator<A> {
    fn new() -> Self {
        Translator { rotation: 0, offset: A::ZERO }
    }

    ///the rotation turn: rotation + 1, offset doubled. `a2p_new ∘ p2a_old`
    ///is the turn's relocation map — names preserved by construction; the
    ///old rotation cancels (`ror r ∘ rol (r+1) = rol 1`), leaving
    ///`pos → 2·pos`, +1 across the name-space wrap (pos < offset).
    fn rotate(&self) -> Self {
        Translator {
            rotation: self.rotation + 1,
            offset:   self.offset.wrapping_add(self.offset),
        }
    }

    #[inline]
    fn a2p(&self, a: A) -> Pos {
        Pos(a.rotate_left(self.rotation).wrapping_add(self.offset).as_usize())
    }

    #[inline]
    fn p2a(&self, p: Pos) -> A {
        A::from_usize(p.0).wrapping_sub(self.offset).rotate_right(self.rotation)
    }
}

///one mutation event, compact. `Empty` = nothing pending (initial state;
///`clear` restores it — a no-op slide is still pending, `iter` just yields
///nothing). the common contiguous run slide (the hole moves `from → to`;
///every element strictly between shifts one step toward `from`). `Pairs`
///covers rare non-run shapes (reversal, bulk).
enum Delta {
    Empty,
    NoneSlide {
        from: Pos,
        to:   Pos,
    },
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}

///expanded `(from, to)` pos pairs of one event, swap-cascade order
///(hole-adjacent mover first). `Run`: `p` cursor, `step` (−1 = run shifts
///up), `remaining` emissions; each pair is `(p, p − step)`.
enum DeltaPairs<'a> {
    Empty,
    Run { p: usize, step: i64, remaining: usize },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}

impl Delta {
    fn pairs(&self) -> DeltaPairs<'_> {
        match self {
            Delta::Empty => DeltaPairs::Empty,
            Delta::NoneSlide { from, to } => DeltaPairs::Run {
                p:         if from.0 > to.0 { from.0 - 1 } else { from.0 + 1 },
                step:      if from.0 > to.0 { -1 } else { 1 },
                remaining: from.0.abs_diff(to.0),
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

///mask covers data.len() slots; bit ⇔ slot initialized.
pub struct BlockSet<K, A> {
    translator: Translator<A>,
    data:       Vec<MaybeUninit<K>>,
    mask:       Mask,
    order:      Direction,
    len:        usize,
    ///last minted addr — density heuristic tier
    #[allow(dead_code)]
    back:       A,
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
        Self::with_bias(InsertBias::Insert)
    }

    pub fn with_bias(bias: InsertBias) -> Self {
        let order = match bias {
            InsertBias::Append | InsertBias::Insert => Direction::Asc,
            InsertBias::Prepend => Direction::Desc,
        };
        BlockSet {
            translator: Translator::new(),
            data: Vec::new(),
            mask: Mask::new(),
            order,
            len: 0,
            back: A::ZERO,
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

    pub fn find(&self, k: &K) -> Option<A> {
        match self.position(k) {
            Ok(p) => Some(self.translator.p2a(p)),
            Err(_) => None,
        }
    }

    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        //Desc blocks: key order ascends against pos order
        let rev = matches!(self.order, Direction::Desc);
        let mut cursor = if rev { self.mask.bits() } else { 0 };
        std::iter::from_fn(move || {
            let p =
                if rev { self.mask.prev_set(cursor)? } else { self.mask.next_set(cursor)? };
            cursor = if rev { p } else { p + 1 };
            Some(self.key(p))
        })
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
            return Ok(self.push_back(k, spot));
        }
        let p = self.open(&k, spot, deltas);
        Ok(self.place(k, p))
    }

    ///insertion spot, or None on dup. beyond the tail extreme the spot is
    ///known without bsearch
    fn spot(&self, k: &K) -> Option<Pos> {
        if self.len > 0 && self.order.beyond_tail(k, self.key_tail()) {
            return Some(Pos(self.mask.last_set().unwrap() + 1));
        }
        self.position(k).err()
    }

    ///vec-like first allocation: 4 slots, no translator turn until it fills
    fn insert_first(&mut self, k: K) -> A {
        self.data = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.mask = Mask::with_bits(4);
        self.place(k, Pos(0))
    }

    ///extend the allocation and place k at the edge, leaving a hole below
    ///k whenever the tail run would span BIT_WIDTH live slots — below the
    ///tail, so sequential inserts can't consume it; middle inserts find
    ///space within budget. caller guarantees spot == cap < max_capacity.
    fn push_back(&mut self, k: K, spot: Pos) -> A {
        let run = self.mask.prev_clear(spot.0 - 1).map_or(spot.0, |h| spot.0 - h - 1);
        let hole = run >= A::BIT_WIDTH as usize - 1;
        if hole {
            self.push_slot();
        }
        self.push_slot();
        self.place(k, spot + usize::from(hole))
    }

    ///open a slot at/near `spot`, cheapest space first: a probe within
    ///BIT_WIDTH; on a miss, the back — if the tail run is within budget the
    ///allocation edge is the nearest space, so open a slot there and slide
    ///the run into it; else a rotation turn in a ≥80%-full block (one O(n)
    ///move with zero name churn beats the long slides and pair cascades
    ///dense probing produces); else a wide slide (a fluke gap in a sparse
    ///block). mask full ⇒ turn, or one plain tail slot when the turn is
    ///refused (room always exists: mask full + len < name ceiling).
    ///heuristic tier later.
    fn open(&mut self, k: &K, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let budget = A::BIT_WIDTH as usize;
        if let Some(p) = self.try_alloc(spot, budget, deltas) {
            return p;
        }
        //budget miss, tail run within budget: the back is the nearest
        //space — open a slot there, the wide probe finds it
        if spot.0 < self.data.len()
            && self.data.len() - spot.0 <= budget
            && self.data.len() < Self::max_capacity()
        {
            self.push_slot();
            return self.try_alloc(spot, self.data.len(), deltas).expect("back slot is nearest");
        }
        //systemic scarcity: dense block, no hole within budget — spread
        if self.len >= self.data.len() - self.data.len() / 5 && self.grow_rotate() {
            let spot = self.position(k).err().expect("no dup post-grow");
            return self.try_alloc(spot, self.data.len(), deltas).expect("grow leaves holes");
        }
        //fluke: a wide slide in a sparse block — unless the block is
        //filling (≥ two thirds, so a spread still leaves it ≥ one third)
        //and the nearest hole is more than four budgets away: a locally
        //dense hot region (global density alone is blind to it)
        if self.len >= self.data.len() - self.data.len() / 3
            && self
                .mask
                .nearest_clear(spot.0)
                .is_some_and(|h| h.abs_diff(spot.0) > 4 * budget)
            && self.grow_rotate()
        {
            let spot = self.position(k).err().expect("no dup post-grow");
            return self.try_alloc(spot, self.data.len(), deltas).expect("grow leaves holes");
        }
        if let Some(p) = self.try_alloc(spot, self.data.len(), deltas) {
            return p;
        }
        //mask full: turn if possible, else one plain tail slot
        let turned = self.grow_rotate();
        if !turned {
            self.push_slot();
        }
        let spot = if turned {
            self.position(k).err().expect("no dup post-grow")
        } else {
            spot
        };
        self.try_alloc(spot, self.data.len(), deltas).expect("grow leaves holes")
    }

    ///write k at an open slot, mark live, mint its name
    fn place(&mut self, k: K, p: Pos) -> A {
        self.data[p.0] = MaybeUninit::new(k);
        self.mask.set(p.0, true);
        self.len += 1;
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
    ///the probe compare known. Desc is just flipped operands (`locate`).
    fn position(&self, k: &K) -> Result<Pos, Pos> {
        let mut lo = 0;
        let mut hi = self.data.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => match self.order.locate(k, self.key(p)) {
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

    ///find the nearest hole within `budget` of `spot` (either side; ties go
    ///right), slide it adjacent, record pairs. fails clean: no mutation on
    ///None. hole left of spot: the run `hole+1..spot` shifts down, k lands on
    ///`spot − 1`; hole right: run `spot..hole` shifts up, k lands on `spot`.
    fn try_alloc(&mut self, spot: Pos, budget: usize, deltas: &mut Deltas<A>) -> Option<Pos> {
        let hole = self.mask.nearest_clear(spot.0).filter(|h| h.abs_diff(spot.0) <= budget)?;
        deltas.translator = self.translator;
        //the run between spot and hole is fully live (nearest hole), so the
        //mask update collapses to: hole fills, the vacated edge clears
        let open = if hole >= spot.0 {
            //ptr::copy leaves a bitwise duplicate in the source range —
            //uninit by mask, never assume_init'd (mask ⇔ init invariant)
            unsafe {
                let ptr = self.data.as_mut_ptr();
                std::ptr::copy(ptr.add(spot.0), ptr.add(spot.0 + 1), hole - spot.0);
            }
            self.mask.set(hole, true);
            self.mask.set(spot.0, false);
            deltas.event = Delta::NoneSlide { from: Pos(hole), to: spot };
            spot
        } else {
            unsafe {
                let ptr = self.data.as_mut_ptr();
                std::ptr::copy(ptr.add(hole + 1), ptr.add(hole), spot.0 - hole - 1);
            }
            self.mask.set(hole, true);
            self.mask.set(spot.0 - 1, false);
            deltas.event = Delta::NoneSlide { from: Pos(hole), to: Pos(spot.0 - 1) };
            Pos(spot.0 - 1)
        };
        Some(open)
    }

    ///rotation turn (in-place): cap doubles; every live element relocates
    ///to `a2p_new(name)` — names preserved by construction, the map
    ///monotone (order holds), each target ≥ its source (the descending
    ///pass writes every target before its source is read). refused past
    ///the name-space midpoint (doubled positions would exceed the address
    ///space) or at the rotation budget; refused while offset ≠ 0 until
    ///offset turns land. mask half: `rotate_mask` — the wrap band's
    ///junction eats the hole the interleave would open (the addr MAX|MIN
    ///pair) or doubles it.
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.data.len();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        if old_cap > A::MIDPOINT.as_usize()
            || self.translator.rotation >= A::BIT_WIDTH as u32
            || self.translator.offset != A::ZERO
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
        self.mask.rotate_mask(self.translator.offset.as_usize(), A::MIDPOINT.as_usize());
        self.translator = new_tr;
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

#[cfg(test)]
mod tests {
    use super::*;

    ///apply translated deltas as name swaps, in the given order
    fn apply(holder: &mut Vec<(u8, u64)>, deltas: &Deltas<u8>) {
        for (from, to) in deltas.iter() {
            for (a, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    fn insert_all(
        keys: impl IntoIterator<Item = u64>,
        bias: InsertBias,
    ) -> (BlockSet<u64, u8>, Vec<(u8, u64)>) {
        let mut set = BlockSet::with_bias(bias);
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

    const BIASES: [InsertBias; 3] =
        [InsertBias::Insert, InsertBias::Append, InsertBias::Prepend];

    #[test]
    fn ascending() {
        for b in BIASES {
            let (s, h) = insert_all(0..200, b);
            check(&s, &h, 200);
        }
    }

    #[test]
    fn descending() {
        for b in BIASES {
            let (s, h) = insert_all((0..200).rev(), b);
            check(&s, &h, 200);
        }
    }

    #[test]
    fn stride() {
        for b in BIASES {
            let (s, h) = insert_all((0..200).map(|i| (i * 37 + 11) % 211), b);
            check(&s, &h, 200);
        }
    }

    ///pure appends push_back instead of turning — cap tracks n, not the next
    ///power of two; the cadence holes sit BELOW the tail (appends can't
    ///consume them), so a spurious middle insert lands in one within budget
    ///— no turn
    #[test]
    fn append_bias_pushes_back_and_keeps_holes() {
        let mut set = BlockSet::<u64, u8>::with_bias(InsertBias::Append);
        let mut d = Deltas::new();
        for k in (0..100u64).map(|i| i * 2) {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        assert!(set.cap() < 128, "cap {} — turned instead of pushing back", set.cap());
        //interior holes at the cadence: a mid-block probe finds one within
        //u8's BIT_WIDTH budget
        let tail_run = set.cap() - set.mask.last_set().unwrap() - 1;
        let holes = set.cap() - set.len();
        assert!(holes > 1, "{holes} holes — cadence consumed");
        assert!(tail_run < 8, "trailing run {tail_run} ate the cadence");
        let before = set.cap();
        set.insert(101, &mut d).unwrap();
        assert_eq!(set.cap(), before, "middle insert turned");
        assert!(d.iter().count() <= 8, "cascaded past budget");
        let keys: Vec<u64> = set.iter().copied().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(set.len(), 101);
    }

    ///reversed block: descending keys push_back at the tail — no front
    ///slides, cap tracks n (ascending keys hit pos 0 via the ascending test)
    #[test]
    fn prepend_bias_pushes_back_descending() {
        let mut set = BlockSet::<u64, u8>::with_bias(InsertBias::Prepend);
        let mut d = Deltas::new();
        for k in (0..100u64).rev() {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        assert!(set.cap() < 128, "cap {} — grew instead of pushing back", set.cap());
        let keys: Vec<u64> = set.iter().copied().collect();
        assert_eq!(keys, (0..100).collect::<Vec<_>>());
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
                let old = Translator::<u8> { rotation: r, offset: o };
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
