//!BlockSet: pos-ordered slot array (pos 0 = min) + 2-knob translator. addr =
//!canonical name of its slot (`p2a`), semi-stable — rotation turns (growth)
//!preserve names, slides transfer them (the mover adopts the vacated name).
//!moves are reported as pos pairs in swap-cascade order (hole-adjacent
//!mover first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.

use std::cmp::Ordering;
use std::mem::MaybeUninit;

use crate::index::*;
use crate::mask::Mask;

///insertion-side preference — heuristic tier (unused)
#[allow(dead_code)]
enum InsertBias {
    Append,
    Prepend,
    Insert,
}

///`a2p(a) = (a rol rotation) + offset`, `p2a` the exact inverse (both wrap in
///A). rotation turn = growth: every live pos maps to `2·pos − offset` mod A.
///offset turn = window move (heuristic tier, unimplemented).
#[derive(Clone, Copy)]
struct Translator<A> {
    rotation: u32,
    offset: A,
}

impl<A: UnsignedNum> Translator<A> {
    fn new() -> Self {
        Translator { rotation: 0, offset: A::ZERO }
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

///one mutation event, compact. the common contiguous run slide (the hole
///moves `from → to`; every element strictly between shifts one step toward
///`from`) — `NoneSlide` with `from == to` is a no-op (insert landed in an
///existing hole). `Pairs` covers rare non-run shapes (reversal, bulk).
enum Delta {
    NoneSlide { from: Pos, to: Pos },
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}

///expanded `(from, to)` pos pairs of one event, swap-cascade order
///(hole-adjacent mover first). `Run`: `p` cursor, `step` (−1 = run shifts
///up), `remaining` emissions; each pair is `(p, p − step)`.
enum DeltaPairs<'a> {
    Run { p: usize, step: i64, remaining: usize },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}

impl Delta {
    fn pairs(&self) -> DeltaPairs<'_> {
        match self {
            Delta::NoneSlide { from, to } => DeltaPairs::Run {
                p: if from.0 > to.0 { from.0 - 1 } else { from.0 + 1 },
                step: if from.0 > to.0 { -1 } else { 1 },
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
///applicable in order. `insert` clears on entry — deltas are per-call.
pub struct Deltas<A> {
    event: Delta,
    translator: Translator<A>,
}

impl<A: UnsignedNum> Deltas<A> {
    pub fn new() -> Self {
        Deltas { event: Delta::NoneSlide { from: Pos(0), to: Pos(0) }, translator: Translator::new() }
    }

    ///(from_addr, to_addr) name swaps, applicable in order
    pub fn iter(&self) -> impl Iterator<Item = (A, A)> + '_ {
        self.event
            .pairs()
            .map(|(f, t)| (self.translator.p2a(f), self.translator.p2a(t)))
    }

    fn clear(&mut self) {
        self.event = Delta::NoneSlide { from: Pos(0), to: Pos(0) };
        self.translator = Translator::new()
    }
}

///mask covers data.len() slots; bit ⇔ slot initialized.
#[allow(dead_code)]
pub struct BlockSet<K, A> {
    translator: Translator<A>,
    data: Vec<MaybeUninit<K>>,
    mask: Mask,
    bias: InsertBias,
    len: usize,
    ///last minted addr — density heuristic tier
    back: A,
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
            data: Vec::new(),
            mask: Mask::new(),
            bias: InsertBias::Insert,
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
        self.mask.iter_set().map(|p| self.key(p))
    }

    ///deltas cleared on entry; Err returns k (dup, or out of names).
    pub fn insert(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        deltas.clear();
        if self.data.is_empty() {
            //vec-like first allocation: 4 slots, no translator turn until it fills
            self.data = (0..4).map(|_| MaybeUninit::uninit()).collect();
            self.mask = Mask::with_bits(4);
            self.data[0] = MaybeUninit::new(k);
            self.mask.set(0, true);
            self.len = 1;
            return Ok(self.translator.p2a(Pos(0)));
        }
        if self.len >= Self::max_capacity() {
            return Err(k);
        }
        let spot = match self.position(&k) {
            Ok(_) => return Err(k),
            Err(p) => p,
        };
        //basics: infinite budget; budget/heuristic tier comes later
        let open = match self.try_alloc(spot, self.data.len(), deltas) {
            Some(p) => p,
            None => {
                //full: rotation turn, then recompute — every live pos moved
                if !self.grow_rotate() {
                    return Err(k);
                }
                let spot = self.position(&k).err().expect("no dup post-grow");
                self.try_alloc(spot, self.data.len(), deltas).expect("grow leaves holes")
            }
        };
        self.data[open.0] = MaybeUninit::new(k);
        self.mask.set(open.0, true);
        self.len += 1;
        Ok(self.translator.p2a(open))
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
    ///insertion spot: first live pos whose key > k, or first pos past all
    ///live < k. holes are skipped forward — sortedness makes everything live
    ///before the probe compare known.
    fn position(&self, k: &K) -> Result<Pos, Pos> {
        let mut lo = 0;
        let mut hi = self.data.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => match self.key(p).cmp(k) {
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

    ///find the nearest hole within `budget` of `spot` (either side; ties go
    ///right), slide it adjacent, record pairs. fails clean: no mutation on
    ///None. hole left of spot: the run `hole+1..spot` shifts down, k lands on
    ///`spot − 1`; hole right: run `spot..hole` shifts up, k lands on `spot`.
    fn try_alloc(&mut self, spot: Pos, budget: usize, deltas: &mut Deltas<A>) -> Option<Pos> {
        let hole = self
            .mask
            .nearest_clear(spot.0)
            .filter(|h| h.abs_diff(spot.0) <= budget)?;
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

    ///rotation turn (in-place): cap doubles, live elements relocate to
    ///`a2p_new(name)` (names preserved); with offset 0 the new pos is exactly
    ///`2·pos` — strictly increasing, so a descending pass writes every target
    ///before its source is read, and the mask spreads word-wise. terminal at
    ///cap == name space. offset turns make the descending assumption unsafe —
    ///they report no turn (false) until that tier lands.
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.data.len();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        if new_cap > Self::max_capacity()
            || self.translator.rotation >= A::BIT_WIDTH as u32
            || self.translator.offset != A::ZERO
        {
            return false;
        }
        self.translator.rotation += 1;
        self.data.reserve(old_cap);
        self.data.extend((old_cap..new_cap).map(|_| MaybeUninit::uninit()));
        for p in (0..old_cap).rev() {
            if self.mask.get(p) {
                self.data[p * 2] = std::mem::replace(&mut self.data[p], MaybeUninit::uninit());
            }
        }
        self.mask.spread_double();
        true
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

    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>) {
        let mut set = BlockSet::new();
        let mut holder = Vec::new();
        let mut deltas = Deltas::new();
        for k in keys {
            match set.insert(k, &mut deltas) {
                Ok(addr) => {
                    apply(&mut holder, &deltas);
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

    #[test]
    fn descending() {
        let (s, h) = insert_all((0..200).rev());
        check(&s, &h, 200);
    }

    #[test]
    fn stride() {
        let (s, h) = insert_all((0..200).map(|i| (i * 37 + 11) % 211));
        check(&s, &h, 200);
    }

    ///front insert below cap: plain slide, name transfer without any turn —
    ///the run 20,10 cascades right (hole-adjacent first), 5 takes name 0
    #[test]
    fn front_insert_renames_min() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(10, &mut d).unwrap();
        set.insert(20, &mut d).unwrap();
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(2), Some(&20));
    }

    ///the canonical rotated front-insert: cap-4 store turns to cap 8 (r=1,
    ///live names 0..3 preserved at pos 0,2,4,6), then the front slide — 10
    ///takes the minted hole name (1 ror 1 = 128), 5 takes the freed name 0.
    #[test]
    fn rotated_front_insert_renames_min() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in [10, 20, 30, 40] {
            set.insert(k, &mut d).unwrap();
        }
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(0, 128)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(128), Some(&10));
        assert_eq!(set.get(1), Some(&20));
        assert_eq!(set.get(2), Some(&30));
        assert_eq!(set.get(3), Some(&40));
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
            vec![(Pos(9), Pos(10)), (Pos(8), Pos(9)), (Pos(7), Pos(8)), (Pos(6), Pos(7)), (Pos(5), Pos(6))],
            "run shifts up"
        );
        assert_eq!(
            expand(5, 9),
            vec![(Pos(6), Pos(5)), (Pos(7), Pos(6)), (Pos(8), Pos(7)), (Pos(9), Pos(8))],
            "run shifts down"
        );
        assert_eq!(expand(7, 7), Vec::<(Pos, Pos)>::new(), "from == to = no-op");
    }

    #[test]
    fn remove_and_reinsert() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in 0..50u64 {
            set.insert(k, &mut d).unwrap();
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