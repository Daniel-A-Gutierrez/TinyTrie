//!BlockBiMap: the bijective tier — two lockstep BlockMaps, each holding the
//!other's names as its values, so both orders are native (no projection
//!ordering) and every pair is one hop from either side. insert churns
//!BOTH name spaces (one event each, in the maps' own `Deltas` slots); the
//!maps consume each other's events by the BACK-POINTER CHASE — a delta
//!pair names the emitter's moved entry, and that entry's own value
//!addresses the one partner slot whose held name went stale (O(1) per
//!pair, no search; bijection guarantees a single partner per pair). the
//!fresh entry's dummy value is never chased before its patch: only its
//!own partner points at it, and a placed entry never moves.
//!cursors (`seek`/`seek_by_value`): blockmap cursors over either native
//!order, the partner chased one hop per yield (like the iters).

use crate::blockmap::BlockMap;
use crate::blockmap::Cursor as MapCursor;
use crate::blockset::Deltas;
use crate::index::*;

///the back-pointer chase: apply `emitter`'s event to `target`'s value
///column, one write per moved entry. a delta pair names the emitter's
///moved entry, and that entry's own value addresses its partner slot in
///target — O(1) per pair, no search (bijection: one partner per entry).
///gather pairs carry the FULL hop (crossing count), so every pair is
///final — a single forward pass, no chained intermediate names. reads
///run in POS space (`next_pos` pulls): the flip fires pre-record, so
///the emitter's translator is unchanged from the record to this call;
///events held across mutations must go through `iter`'s names instead
///(era-stable — subtle_bugs §22)
fn chase<EK: Ord, TK: Ord, A: UnsignedNum>(
    emitter: &mut BlockMap<EK, A, A>,
    target: &mut BlockMap<TK, A, A>,
) {
    while let Some((from, to)) = emitter.deltas().next_chase() {
        let partner = *emitter.value_at_pos(to).expect("moved entry live");
        //the partner held the entry's ORIGINAL name before the event
        debug_assert_eq!(*target.get(partner).expect("partner live").1, emitter.name_at(from));
        target.set_value(partner, emitter.name_at(to));
    }
}

///bijective map: K and V both unique, each entry reachable from either
///side in one hop. insert returns the pair's (K-side, V-side) names —
///both are ordinary `Deltas` names; removals move nothing (no deltas)
pub struct BlockBiMap<K, V, A> {
    ///K → the entry's V-side name
    keys:   BlockMap<K, A, A>,
    ///V → the entry's K-side name
    values: BlockMap<V, A, A>,
}

impl<K: Ord, V: Ord, A: UnsignedNum> BlockBiMap<K, V, A> {
    pub fn new() -> Self {
        BlockBiMap { keys: BlockMap::new(), values: BlockMap::new() }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn max_capacity() -> usize {
        A::MAX.as_usize().saturating_add(1)
    }

    ///entry by K-side name
    pub fn get_at(&self, k_addr: A) -> Option<(&K, &V)> {
        let (k, &val_addr) = self.keys.get(k_addr)?;
        let (v, _) = self.values.get(val_addr).expect("bijection: partner live");
        Some((k, v))
    }

    ///entry by V-side name
    pub fn get_at_value(&self, val_addr: A) -> Option<(&V, &K)> {
        let (v, &k_addr) = self.values.get(val_addr)?;
        let (k, _) = self.keys.get(k_addr).expect("bijection: partner live");
        Some((v, k))
    }

    pub fn get_by_key(&self, k: &K) -> Option<(&K, &V)> {
        self.keys.find(k).and_then(|a| self.get_at(a))
    }

    pub fn get_by_value(&self, v: &V) -> Option<(&V, &K)> {
        self.values.find(v).and_then(|a| self.get_at_value(a))
    }

    pub fn key_addr(&self, k: &K) -> Option<A> {
        self.keys.find(k)
    }

    pub fn value_addr(&self, v: &V) -> Option<A> {
        self.values.find(v)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.keys.contains(k)
    }

    pub fn contains_value(&self, v: &V) -> bool {
        self.values.contains(v)
    }

    ///first→last in key order
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        self.keys.iter().map(|(k, &val_addr)| {
            let (v, _) = self.values.get(val_addr).expect("bijection: partner live");
            (k, v)
        })
    }

    ///first→last in value order
    pub fn iter_by_value(&self) -> impl Iterator<Item = (&V, &K)> + '_ {
        self.values.iter().map(|(v, &k_addr)| {
            let (k, _) = self.keys.get(k_addr).expect("bijection: partner live");
            (v, k)
        })
    }

    ///cursor at the first key ≥ k, or the walk's end if none
    pub fn seek(&self, k: &K) -> Cursor<'_, K, V, A> {
        Cursor { bimap: self, walk: self.keys.seek(k) }
    }

    ///cursor at the first key > k — k itself lands in a downward walk
    pub fn seek_after(&self, k: &K) -> Cursor<'_, K, V, A> {
        Cursor { bimap: self, walk: self.keys.seek_after(k) }
    }

    ///cursor at the first value ≥ v (value order), or the walk's end
    pub fn seek_by_value(&self, v: &V) -> ValueCursor<'_, K, V, A> {
        ValueCursor { bimap: self, walk: self.values.seek(v) }
    }

    ///cursor at the first value > v (value order) — v lands in a
    ///downward walk
    pub fn seek_after_by_value(&self, v: &V) -> ValueCursor<'_, K, V, A> {
        ValueCursor { bimap: self, walk: self.values.seek_after(v) }
    }

    ///Err returns (k, v) — dup on either side, or a full name space. a
    ///K-SIDE Err mutates no entries and records no deltas — the
    ///orientation heuristic may still have flipped the block (names
    /// unaffected, invisible through the direction-aware API); a dup-V
    ///Err rolled the just-placed k back, so the slides it caused were
    ///real — key-side deltas may be PENDING: apply + clear on Err like
    ///on Ok
    pub fn insert(&mut self, k: K, v: V) -> Result<(A, A), (K, V)> {
        assert!(
            !self.keys.deltas().is_pending(),
            "pending deltas: apply + exhaust before reuse (keys)"
        );
        assert!(
            !self.values.deltas().is_pending(),
            "pending deltas: apply + exhaust before reuse (values)"
        );
        //K first, dummy value — its only reader is the patch below. dup k
        //or a full name space ⇒ Err before any mutation
        let k_addr = match self.keys.insert(k, A::ZERO) {
            Ok(a) => a,
            Err((k, _)) => return Err((k, v)),
        };
        chase(&mut self.keys, &mut self.values);
        //the one Err-after-mutation path: values' capacity can't differ
        //(lens always equal), so this is a value dup — roll the placed k
        //back; the slides it caused stay (kdeltas pending)
        let val_addr = match self.values.insert(v, k_addr) {
            Ok(a) => a,
            Err((v, _)) => {
                let k = match self.keys.remove(k_addr) {
                    Some((k, _)) => k,
                    None => unreachable!("just placed"),
                };
                return Err((k, v));
            }
        };
        chase(&mut self.values, &mut self.keys);
        self.keys.set_value(k_addr, val_addr);
        Ok((k_addr, val_addr))
    }

    ///both event slots, (K-side, V-side) — the two-stream deltas surface
    pub fn deltas(&mut self) -> (&mut Deltas<A>, &mut Deltas<A>) {
        (self.keys.deltas(), self.values.deltas())
    }

    pub fn remove_by_key(&mut self, k: &K) -> Option<(K, V)> {
        let k_addr = self.keys.find(k)?;
        self.remove_pair(k_addr)
    }

    pub fn remove_by_value(&mut self, v: &V) -> Option<(K, V)> {
        let val_addr = self.values.find(v)?;
        let (_, &k_addr) = self.values.get(val_addr)?;
        self.remove_pair(k_addr)
    }

    ///drop the pair by K-side name; the back-pointer asserts ARE the
    ///bijection check
    fn remove_pair(&mut self, k_addr: A) -> Option<(K, V)> {
        let (_, &val_addr) = self.keys.get(k_addr)?;
        let (v, back) = self.values.remove(val_addr).expect("bijection: partner live");
        debug_assert_eq!(back, k_addr);
        let (k, back) = self.keys.remove(k_addr).expect("bijection: partner live");
        debug_assert_eq!(back, val_addr);
        Some((k, v))
    }
}

///key-ordered bidirectional walk — a blockmap cursor over the keys side,
///the partner chased one hop per yield (see blockmap::Cursor; invalidated
///by any mutation — re-seek)
pub struct Cursor<'a, K, V, A> {
    bimap: &'a BlockBiMap<K, V, A>,
    walk:  MapCursor<'a, K, A, A>,
}

///value-ordered bidirectional walk — Cursor's mirror over the values
///side, yields (&V, &K)
pub struct ValueCursor<'a, K, V, A> {
    bimap: &'a BlockBiMap<K, V, A>,
    walk:  MapCursor<'a, V, A, A>,
}

impl<'a, K: Ord, V: Ord, A: UnsignedNum> Cursor<'a, K, V, A> {
    ///reposition at the first key ≥ k
    pub fn seek(&mut self, k: &K) {
        self.walk.seek(k);
    }

    ///reposition at the first key > k (a dup at k skipped)
    pub fn seek_after(&mut self, k: &K) {
        self.walk.seek_after(k);
    }

    ///yield the current entry, step forward (key-ascending). None at the
    ///walk's end
    #[inline]
    pub fn next(&mut self) -> Option<(&'a K, &'a V)> {
        let (k, &val_addr) = self.walk.next()?;
        let (v, _) = self.bimap.values.get(val_addr).expect("bijection: partner live");
        Some((k, v))
    }

    ///step back (key-descending), yield the entry now current. None at
    ///the walk's start; from the forward ghost, resumes at the last entry
    #[inline]
    pub fn prev(&mut self) -> Option<(&'a K, &'a V)> {
        let (k, &val_addr) = self.walk.prev()?;
        let (v, _) = self.bimap.values.get(val_addr).expect("bijection: partner live");
        Some((k, v))
    }

    ///the entry next() would yield, without moving
    pub fn entry(&self) -> Option<(&'a K, &'a V)> {
        let (k, &val_addr) = self.walk.entry()?;
        let (v, _) = self.bimap.values.get(val_addr).expect("bijection: partner live");
        Some((k, v))
    }

    ///the current entry's names, (K-side, V-side)
    pub fn addrs(&self) -> Option<(A, A)> {
        let (_, &val_addr) = self.walk.entry()?;
        Some((self.walk.addr()?, val_addr))
    }
}

impl<'a, K: Ord, V: Ord, A: UnsignedNum> ValueCursor<'a, K, V, A> {
    ///reposition at the first value ≥ v
    pub fn seek(&mut self, v: &V) {
        self.walk.seek(v);
    }

    ///reposition at the first value > v (a dup at v skipped)
    pub fn seek_after(&mut self, v: &V) {
        self.walk.seek_after(v);
    }

    ///yield the current entry, step forward (value-ascending). None at
    ///the walk's end
    #[inline]
    pub fn next(&mut self) -> Option<(&'a V, &'a K)> {
        let (v, &k_addr) = self.walk.next()?;
        let (k, _) = self.bimap.keys.get(k_addr).expect("bijection: partner live");
        Some((v, k))
    }

    ///step back (value-descending), yield the entry now current. None at
    ///the walk's start; from the forward ghost, resumes at the last entry
    #[inline]
    pub fn prev(&mut self) -> Option<(&'a V, &'a K)> {
        let (v, &k_addr) = self.walk.prev()?;
        let (k, _) = self.bimap.keys.get(k_addr).expect("bijection: partner live");
        Some((v, k))
    }

    ///the entry next() would yield, without moving
    pub fn entry(&self) -> Option<(&'a V, &'a K)> {
        let (v, &k_addr) = self.walk.entry()?;
        let (k, _) = self.bimap.keys.get(k_addr).expect("bijection: partner live");
        Some((v, k))
    }

    ///the current entry's names, (K-side, V-side) — Cursor::addrs' order
    pub fn addrs(&self) -> Option<(A, A)> {
        let (_, &k_addr) = self.walk.entry()?;
        Some((k_addr, self.walk.addr()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///one cursor order's battery (see blockset::assert_walks): live =
    ///sorted live (order-key, partner) pairs — every boundary × both
    ///seeks × both directions, addrs resolve both ways, ghost round-trip.
    ///$swap: the walk pairs are (value, key) — addrs orients them
    macro_rules! cursor_battery {
        ($map:expr, $live:expr, $seek:ident, $seek_after:ident, $swap:expr) => {{
            let live: Vec<(u64, u64)> = $live;
            let max = live.last().unwrap().0 + 1;
            for k in 0..=max {
                let mut c = $map.$seek(&k);
                let peek = live.iter().find(|(x, _)| *x >= k).copied();
                assert_eq!(c.entry().map(|(x, y)| (*x, *y)), peek, "peek {k}");
                if let (Some((x, y)), Some((ka, va))) = (peek, c.addrs()) {
                    let (kk, vv) = if $swap { (y, x) } else { (x, y) };
                    assert_eq!($map.get_at(ka), Some((&kk, &vv)), "addrs key-side {k}");
                    assert_eq!($map.get_at_value(va), Some((&vv, &kk)), "addrs value-side {k}");
                }
                let expect: Vec<(u64, u64)> =
                    live.iter().copied().filter(|(x, _)| *x >= k).collect();
                let got: Vec<(u64, u64)> =
                    std::iter::from_fn(|| c.next().map(|(x, y)| (*x, *y))).collect();
                assert_eq!(got, expect, concat!(stringify!($seek), "({}) forward"), k);
                let mut c = $map.$seek(&k);
                let expect: Vec<(u64, u64)> =
                    live.iter().rev().copied().filter(|(x, _)| *x < k).collect();
                let got: Vec<(u64, u64)> =
                    std::iter::from_fn(|| c.prev().map(|(x, y)| (*x, *y))).collect();
                assert_eq!(got, expect, concat!(stringify!($seek), "({}) back"), k);
                let mut c = $map.$seek_after(&k);
                let expect: Vec<(u64, u64)> =
                    live.iter().copied().filter(|(x, _)| *x > k).collect();
                let got: Vec<(u64, u64)> =
                    std::iter::from_fn(|| c.next().map(|(x, y)| (*x, *y))).collect();
                assert_eq!(got, expect, concat!(stringify!($seek_after), "({}) forward"), k);
                let mut c = $map.$seek_after(&k);
                let expect: Vec<(u64, u64)> =
                    live.iter().rev().copied().filter(|(x, _)| *x <= k).collect();
                let got: Vec<(u64, u64)> =
                    std::iter::from_fn(|| c.prev().map(|(x, y)| (*x, *y))).collect();
                assert_eq!(got, expect, concat!(stringify!($seek_after), "({}) back"), k);
            }
            //round-trip across the FwdEnd ghost; exhaustion stable, both
            //sides resume forward correctly
            let mut c = $map.$seek(&live[0].0);
            for kv in &live {
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
        }};
    }

    ///apply both streams to the holder's two name columns, in order
    ///(pulling exhausts — the holder mirror IS the application)
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64, u64, A)>, map: &mut BlockBiMap<u64, u64, A>) {
        let (keys, values) = map.deltas();
        for (from, to) in keys.iter() {
            for (a, _, _, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
        for (from, to) in values.iter() {
            for (_, _, _, a) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    ///drain both event slots without applying (the no-op direct-place
    ///events pend too — a fresh block's appends land in past-tail holes)
    fn clear<A: UnsignedNum>(map: &mut BlockBiMap<u64, u64, A>) {
        let (keys, values) = map.deltas();
        keys.clear();
        values.clear();
    }

    fn insert_all(
        entries: impl IntoIterator<Item = (u64, u64)>,
    ) -> (BlockBiMap<u64, u64, u8>, Vec<(u8, u64, u64, u8)>) {
        let mut map = BlockBiMap::new();
        let mut holder = Vec::new();
        for (k, v) in entries {
            match map.insert(k, v) {
                Ok((ka, va)) => {
                    apply(&mut holder, &mut map);
                    holder.push((ka, k, v, va));
                }
                Err((k, v)) => panic!("insert ({k}, {v}) failed"),
            }
        }
        (map, holder)
    }

    ///both iters strictly monotone, every holder name verified both ways
    fn check(map: &BlockBiMap<u64, u64, u8>, holder: &[(u8, u64, u64, u8)], count: usize) {
        assert_eq!(map.len(), count);
        assert_eq!(holder.len(), count);
        let mut prev = None;
        for (k, _) in map.iter() {
            if let Some(p) = prev {
                assert!(*k > p, "key order broken");
            }
            prev = Some(*k);
        }
        let mut prev = None;
        for (v, _) in map.iter_by_value() {
            if let Some(p) = prev {
                assert!(*v > p, "value order broken");
            }
            prev = Some(*v);
        }
        for (ka, k, v, va) in holder {
            assert_eq!(map.get_at(*ka), Some((k, v)), "holder k-name {ka} broken");
            assert_eq!(map.get_at_value(*va), Some((v, k)), "holder v-name {va} broken");
        }
    }

    #[test]
    fn ascending() {
        let (m, h) = insert_all((0..200).map(|k| (k, k * 3 + 1)));
        check(&m, &h, 200);
    }

    ///k descends (keys flips), v ascends (values appends) — mixed
    /// internal directions, both iters still monotone
    #[test]
    fn descending_keys_ascending_values() {
        let (m, h) = insert_all((0..200).rev().map(|k| (k, 200 - k)));
        check(&m, &h, 200);
        assert!(
            m.iter_by_value().take(3).map(|(v, _)| *v).collect::<Vec<_>>() == vec![1, 2, 3]
        );
    }

    #[test]
    fn stride() {
        let (m, h) = insert_all((0..200).map(|i| ((i * 37 + 11) % 211, (i * 53 + 7) % 211)));
        check(&m, &h, 200);
    }

    ///random removals by either side, clustered + descending-bait streams;
    ///every holder name verified after every op — the chase's torture
    #[test]
    fn mixed_ops_torture() {
        let mut map = BlockBiMap::<u64, u64, u16>::new();
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
                let v = k.wrapping_mul(3) + 7;
                match map.insert(k, v) {
                    Ok((ka, va)) => {
                        apply(&mut holder, &mut map);
                        holder.push((ka, k, v, va));
                    }
                    //a dup-v Err may carry real key-side slides — the
                    //holder applies them like an Ok
                    Err(_) => {
                        apply(&mut holder, &mut map);
                    }
                }
            } else {
                let j = (rnd() % holder.len() as u64) as usize;
                let (ka, k, v, _) = holder[j];
                if rnd() % 2 == 0 {
                    assert_eq!(map.remove_by_key(&k), Some((k, v)));
                } else {
                    assert_eq!(map.remove_by_value(&v), Some((k, v)));
                }
                let _ = ka;
                holder.swap_remove(j);
            }
            for &(ka, k, v, va) in &holder {
                assert_eq!(map.get_at(ka), Some((&k, &v)), "op {i}: k-name {ka} broken");
                assert_eq!(map.get_at_value(va), Some((&v, &k)), "op {i}: v-name {va} broken");
            }
        }
        assert_eq!(map.len(), holder.len());
    }

    #[test]
    fn dups_rejected() {
        let mut map = BlockBiMap::<u64, u64, u8>::new();
        map.insert(1, 11).unwrap();
        clear(&mut map);
        assert_eq!(map.insert(1, 99), Err((1, 99)), "dup key");
        //the k-placement records a no-op event even for a direct place
        //into a hole — pending, but nothing moved (the general dup-v case
        //is the rollback test below)
        assert_eq!(map.insert(2, 11), Err((2, 11)), "dup value");
        assert_eq!(map.deltas().0.iter().count(), 0, "nothing moved");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get_by_key(&1), Some((&1, &11)), "rejected insert must not overwrite");
    }

    ///the dup-v Err-after-mutation path: the placed k slid real entries
    /// before the value dup surfaced — rollback removes the pair, the
    /// slides stay, key-side deltas pending on the Err
    #[test]
    fn dup_value_rolls_back_with_pending_deltas() {
        let mut map = BlockBiMap::<u64, u64, u16>::new();
        for i in 0..10u64 {
            map.insert(i * 10, i * 10 + 100).unwrap();
            clear(&mut map);
        }
        for k in 200..=214u64 {
            map.insert(k, k + 100).unwrap();
            clear(&mut map);
        }
        //holes at pos 8, 9 (7 from the spot of key 5) and 10..19 (drops
        //density below the 3/4 turn line so the slide runs)
        map.remove_by_key(&90).unwrap();
        map.remove_by_key(&80).unwrap();
        for k in 200..=209u64 {
            map.remove_by_key(&k).unwrap();
        }
        //key 5 lands at the occupied pos 1, hole 7 away ⇒ a 7-element
        //slide; value 110 already belongs to key 10 ⇒ dup v
        assert_eq!(map.insert(5, 110), Err((5, 110)));
        assert!(map.deltas().0.is_pending(), "the slide was real");
        assert_eq!(map.deltas().0.iter().count(), 7, "7 elements shifted one step");
        assert!(!map.deltas().1.is_pending(), "the failing insert recorded nothing");
        clear(&mut map);
        assert_eq!(map.len(), 13);
        assert_eq!(map.key_addr(&5), None, "rolled back");
        assert_eq!(map.get_by_value(&110), Some((&110, &10)));
        //bijection intact: every surviving pair resolves both ways
        for k in (0..8u64).map(|i| i * 10).chain(210..=214) {
            let v = k + 100;
            assert_eq!(map.get_by_key(&k), Some((&k, &v)), "pair {k}");
            assert_eq!(map.get_by_value(&v), Some((&v, &k)), "pair {k}");
        }
    }

    ///insert leaves a pending (no-op) event — the next insert must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas() {
        let mut map = BlockBiMap::<u64, u64, u8>::new();
        map.insert(1, 11).unwrap();
        map.insert(2, 12).unwrap();
        map.insert(3, 13).unwrap();
    }

    ///u8 names: 256 pairs fill the space, the 257th fails clean, a
    ///removal reopens it — no partial state anywhere
    #[test]
    fn name_space_exhaustion() {
        let mut map = BlockBiMap::<u64, u64, u8>::new();
        for k in 0..256u64 {
            map.insert(k, k * 1000).unwrap();
            clear(&mut map);
        }
        assert_eq!(map.len(), 256);
        assert_eq!(map.insert(1000, 5), Err((1000, 5)), "name space full");
        map.remove_by_key(&0).unwrap();
        map.insert(500, 500_000).unwrap();
        clear(&mut map);
        assert_eq!(map.len(), 256);
    }

    ///cursors over both native orders: keys descend (the keys map flips),
    ///values ascend — the orders disagree and the internal directions
    ///are mixed. every boundary × both seeks × both directions, addrs
    ///resolve both ways
    #[test]
    fn cursor_both_orders() {
        //sized for miri — the battery sweeps every boundary regardless of n
        let (mut map, _) = insert_all((0..60u64).rev().map(|k| (k, 60 - k)));
        let removed = [3u64, 17, 42];
        for k in removed {
            map.remove_by_key(&k);
        }
        let by_key: Vec<(u64, u64)> = (0..60u64)
            .filter(|k| !removed.contains(k))
            .map(|k| (k, 60 - k))
            .collect();
        let mut by_value: Vec<(u64, u64)> = by_key.iter().map(|&(k, v)| (v, k)).collect();
        by_value.sort_unstable();
        cursor_battery!(map, by_key, seek, seek_after, false);
        cursor_battery!(map, by_value, seek_by_value, seek_after_by_value, true);
    }

    #[test]
    fn cursor_empty_and_single() {
        let map = BlockBiMap::<u64, u64, u8>::new();
        let mut c = map.seek(&5);
        assert_eq!(c.entry(), None);
        assert_eq!(c.addrs(), None);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), None);
        let mut c = map.seek_by_value(&5);
        assert_eq!(c.entry(), None);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), None);
        let mut map = map;
        map.insert(7, 70).unwrap();
        let mut c = map.seek(&5);
        assert_eq!(c.next(), Some((&7, &70)));
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some((&7, &70)));
        assert_eq!(
            c.addrs(),
            Some((map.key_addr(&7).unwrap(), map.value_addr(&70).unwrap()))
        );
        let mut c = map.seek_by_value(&5);
        assert_eq!(c.next(), Some((&70, &7)));
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some((&70, &7)));
        let mut c = map.seek_after_by_value(&70);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some((&70, &7)));
    }
}
