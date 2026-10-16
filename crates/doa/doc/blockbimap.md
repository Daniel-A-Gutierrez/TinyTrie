```rust
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
//!cursors (`seek`/`seek_by_value`): blockmap cursors over either native
//!order, the partner chased one hop per yield (like the iters).
///L0020
///two event streams, one per name space — apply + clear both like `Deltas`
pub struct BiDeltas<A> {
    ///K-side name swaps
    pub keys:   Deltas<A>,
    ///V-side name swaps
    pub values: Deltas<A>,
}
///L0027
impl<A: UnsignedNum> BiDeltas<A> {}
///L0053
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
);
///L0069
///bijective map: K and V both unique, each entry reachable from either
///side in one hop. insert returns the pair's (K-side, V-side) names —
///both are ordinary `Deltas` names; removals move nothing (no deltas)
pub struct BlockBiMap<K, V, A> {
    ///K → the entry's V-side name
    keys:   BlockMap<K, A, A>,
    ///V → the entry's K-side name
    values: BlockMap<V, A, A>,
}
///L0076
impl<K: Ord, V: Ord, A: UnsignedNum> BlockBiMap<K, V, A> {}
///L0224
///key-ordered bidirectional walk — a blockmap cursor over the keys side,
///the partner chased one hop per yield (see blockmap::Cursor; invalidated
///by any mutation — re-seek)
pub struct Cursor<'a, K, V, A> {
    bimap: &'a BlockBiMap<K, V, A>,
    walk:  MapCursor<'a, K, A, A>,
}
///L0231
///value-ordered bidirectional walk — Cursor's mirror over the values
///side, yields (&V, &K)
pub struct ValueCursor<'a, K, V, A> {
    bimap: &'a BlockBiMap<K, V, A>,
    walk:  MapCursor<'a, V, A, A>,
}
///L0236
impl<'a, K: Ord, V: Ord, A: UnsignedNum> Cursor<'a, K, V, A> {}
///L0279
impl<'a, K: Ord, V: Ord, A: UnsignedNum> ValueCursor<'a, K, V, A> {}
///L0323
#[cfg(test)]
mod tests {
    use super::*;
    ///one cursor order's battery (see blockset::assert_walks): live =
    ///sorted live (order-key, partner) pairs — every boundary × both
    ///seeks × both directions, addrs resolve both ways, ghost round-trip.
    ///$swap: the walk pairs are (value, key) — addrs orients them
    macro_rules! cursor_battery;
    ///apply both streams to the holder's two name columns, in order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64, u64, A)>, d: &BiDeltas<A>);
    fn insert_all(
        entries: impl IntoIterator<Item = (u64, u64)>,
    ) -> (BlockBiMap<u64, u64, u8>, Vec<(u8, u64, u64, u8)>);
    ///both iters strictly monotone, every holder name verified both ways
    fn check(map: &BlockBiMap<u64, u64, u8>, holder: &[(u8, u64, u64, u8)], count: usize);
    #[test]
    fn ascending();
    ///k descends (keys flips), v ascends (values appends) — mixed
    /// internal directions, both iters still monotone
    #[test]
    fn descending_keys_ascending_values();
    #[test]
    fn stride();
    ///random removals by either side, clustered + descending-bait streams;
    ///every holder name verified after every op — the chase's torture
    #[test]
    fn mixed_ops_torture();
    #[test]
    fn dups_rejected();
    ///the dup-v Err-after-mutation path: the placed k slid real entries
    /// before the value dup surfaced — rollback removes the pair, the
    /// slides stay, key-side deltas pending on the Err
    #[test]
    fn dup_value_rolls_back_with_pending_deltas();
    ///insert leaves a pending (no-op) event — the next insert must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas();
    ///u8 names: 256 pairs fill the space, the 257th fails clean, a
    ///removal reopens it — no partial state anywhere
    #[test]
    fn name_space_exhaustion();
    ///cursors over both native orders: keys descend (the keys map flips),
    ///values ascend — the orders disagree and the internal directions
    ///are mixed. every boundary × both seeks × both directions, addrs
    ///resolve both ways
    #[test]
    fn cursor_both_orders();
    #[test]
    fn cursor_empty_and_single();
}
```
