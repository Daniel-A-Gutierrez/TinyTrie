//!BlockBiMap: the bijective tier — two lockstep BlockMaps, each holding the
//!other's names as its values, so both orders are native (no projection
//!ordering) and every pair is one hop from either side. insert churns
//!BOTH name spaces (one event each, carried by `BiDeltas`); the maps
//!consume each other's events by the BACK-POINTER CHASE — a delta pair
//!names the emitter's moved entry, and that entry's own value addresses
//!the one partner slot whose held name went stale (O(1) per pair, no
//!search; bijection guarantees a single partner per pair). the fresh
//!entry's dummy value is never chased before its patch: only its own
//!partner points at it, and a placed entry never moves.

use crate::blockmap::BlockMap;
use crate::blockset::Deltas;
use crate::index::*;

///two event streams, one per name space — apply + clear both like `Deltas`
pub struct BiDeltas<A> {
    ///K-side name swaps
    pub keys:   Deltas<A>,
    ///V-side name swaps
    pub values: Deltas<A>,
}

impl<A: UnsignedNum> BiDeltas<A> {
    pub fn new() -> Self {
        BiDeltas { keys: Deltas::new(), values: Deltas::new() }
    }

    ///both streams empty
    pub(crate) fn is_pending(&self) -> bool {
        self.keys.is_pending() || self.values.is_pending()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.values.clear();
    }
}

///the back-pointer chase: apply `emitter`'s event to `target`'s value
///column, one write per moved entry. a delta pair names the emitter's
///moved entry, and that entry's own value addresses its partner slot in
///target — O(1) per pair, no search (bijection: one partner per entry).
///gather pairs carry the FULL hop (crossing count), so every pair is
///final — a single forward pass, no chained intermediate names. reads
///run in POS space (`iter_pos`): the flip fires pre-record, so the
///emitter's translator is unchanged from the record to this call; events
///held across mutations must go through `iter`'s names instead
///(era-stable — subtle_bugs §22)
fn chase<EK: Ord, TK: Ord, A: UnsignedNum>(
    emitter: &BlockMap<EK, A, A>,
    event: &Deltas<A>,
    target: &mut BlockMap<TK, A, A>,
) {
    for (from, to) in event.iter_pos() {
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

    ///Err returns (k, v) — dup on either side, or a full name space. a
    ///K-SIDE Err mutates no entries and records no deltas — the
    ///orientation heuristic may still have flipped the block (names
    /// unaffected, invisible through the direction-aware API); a dup-V
    ///Err rolled the just-placed k back, so the slides it caused were
    ///real — key-side deltas may be PENDING: apply + clear on Err like
    ///on Ok
    pub fn insert(&mut self, k: K, v: V, deltas: &mut BiDeltas<A>) -> Result<(A, A), (K, V)> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        //K first, dummy value — its only reader is the patch below. dup k
        //or a full name space ⇒ Err before any mutation
        let k_addr = match self.keys.insert(k, A::ZERO, &mut deltas.keys) {
            Ok(a) => a,
            Err((k, _)) => return Err((k, v)),
        };
        chase(&self.keys, &deltas.keys, &mut self.values);
        //the one Err-after-mutation path: values' capacity can't differ
        //(lens always equal), so this is a value dup — roll the placed k
        //back; the slides it caused stay (kdeltas pending)
        let val_addr = match self.values.insert(v, k_addr, &mut deltas.values) {
            Ok(a) => a,
            Err((v, _)) => {
                let k = match self.keys.remove(k_addr) {
                    Some((k, _)) => k,
                    None => unreachable!("just placed"),
                };
                return Err((k, v));
            }
        };
        chase(&self.values, &deltas.values, &mut self.keys);
        self.keys.set_value(k_addr, val_addr);
        Ok((k_addr, val_addr))
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

#[cfg(test)]
mod tests {
    use super::*;

    ///apply both streams to the holder's two name columns, in order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64, u64, A)>, d: &BiDeltas<A>) {
        for (from, to) in d.keys.iter() {
            for (a, _, _, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
        for (from, to) in d.values.iter() {
            for (_, _, _, a) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    fn insert_all(
        entries: impl IntoIterator<Item = (u64, u64)>,
    ) -> (BlockBiMap<u64, u64, u8>, Vec<(u8, u64, u64, u8)>) {
        let mut map = BlockBiMap::new();
        let mut holder = Vec::new();
        let mut d = BiDeltas::new();
        for (k, v) in entries {
            match map.insert(k, v, &mut d) {
                Ok((ka, va)) => {
                    apply(&mut holder, &d);
                    d.clear();
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
        let mut d = BiDeltas::new();
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
                match map.insert(k, v, &mut d) {
                    Ok((ka, va)) => {
                        apply(&mut holder, &d);
                        d.clear();
                        holder.push((ka, k, v, va));
                    }
                    //a dup-v Err may carry real key-side slides — the
                    //holder applies them like an Ok
                    Err(_) => {
                        apply(&mut holder, &d);
                        d.clear();
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
        let mut d = BiDeltas::new();
        map.insert(1, 11, &mut d).unwrap();
        d.clear();
        assert_eq!(map.insert(1, 99, &mut d), Err((1, 99)), "dup key");
        //the k-placement records a no-op event even for a direct place
        //into a hole — pending, but nothing moved (the general dup-v case
        //is the rollback test below)
        assert_eq!(map.insert(2, 11, &mut d), Err((2, 11)), "dup value");
        assert_eq!(d.keys.iter().count(), 0, "nothing moved");
        d.clear();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get_by_key(&1), Some((&1, &11)), "rejected insert must not overwrite");
    }

    ///the dup-v Err-after-mutation path: the placed k slid real entries
    /// before the value dup surfaced — rollback removes the pair, the
    /// slides stay, key-side deltas pending on the Err
    #[test]
    fn dup_value_rolls_back_with_pending_deltas() {
        let mut map = BlockBiMap::<u64, u64, u16>::new();
        let mut d = BiDeltas::new();
        for i in 0..10u64 {
            map.insert(i * 10, i * 10 + 100, &mut d).unwrap();
            d.clear();
        }
        for k in 200..=214u64 {
            map.insert(k, k + 100, &mut d).unwrap();
            d.clear();
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
        assert_eq!(map.insert(5, 110, &mut d), Err((5, 110)));
        assert!(d.keys.is_pending(), "the slide was real");
        assert_eq!(d.keys.iter().count(), 7, "7 elements shifted one step");
        assert!(!d.values.is_pending(), "the failing insert recorded nothing");
        d.clear();
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
        let mut d = BiDeltas::new();
        map.insert(1, 11, &mut d).unwrap();
        map.insert(2, 12, &mut d).unwrap();
        map.insert(3, 13, &mut d).unwrap();
    }

    ///u8 names: 256 pairs fill the space, the 257th fails clean, a
    ///removal reopens it — no partial state anywhere
    #[test]
    fn name_space_exhaustion() {
        let mut map = BlockBiMap::<u64, u64, u8>::new();
        let mut d = BiDeltas::new();
        for k in 0..256u64 {
            map.insert(k, k * 1000, &mut d).unwrap();
            d.clear();
        }
        assert_eq!(map.len(), 256);
        assert_eq!(map.insert(1000, 5, &mut d), Err((1000, 5)), "name space full");
        map.remove_by_key(&0).unwrap();
        map.insert(500, 500_000, &mut d).unwrap();
        d.clear();
        assert_eq!(map.len(), 256);
    }
}