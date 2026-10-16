```rust
//!BlockMap: BlockSet's substrate with a value riding beside every key —
//!two parallel `MaybeUninit` vecs (keys, values) under one shared mask.
//!moves are the blockset ladder (spot / push_back / gather / turn), each
//!relocating the (key, value) pair; the events are the same `Deltas`
//!protocol (addr names — value slots are nameless, a name finds the pair).
//!order: pos order == key order — pos 0 = min key (Asc) or max (Desc);
//!the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): key-ordered walk over live pairs,
//!blockset::Cursor's machine (see blockset).
///L0021
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
///L0031
impl<K, V, A> Drop for BlockMap<K, V, A> {}
///L0043
impl<K: Ord, V, A: UnsignedNum> BlockMap<K, V, A> {}
///L0588
///`BlockMap::iter`'s return type — direction picked at construction
pub enum Iter<'a, K, V, A> {
    Asc { bits: SetBits<'a>, map: &'a BlockMap<K, V, A> },
    Desc { bits: SetBitsRev<'a>, map: &'a BlockMap<K, V, A> },
}
///L0593
impl<'a, K: Ord, V, A: UnsignedNum> Iterator for Iter<'a, K, V, A> {}
///L0609
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
///L0619
impl<'a, K, V, A: UnsignedNum> Cursor<'a, K, V, A> {}
///L0630
impl<'a, K: Ord, V, A: UnsignedNum> Cursor<'a, K, V, A> {}
///L0734
#[cfg(test)]
mod tests {
    use super::*;
    ///apply translated deltas as name swaps, in the given order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64, u64)>, deltas: &Deltas<A>);
    fn insert_all(
        entries: impl IntoIterator<Item = (u64, u64)>,
    ) -> (BlockMap<u64, u64, u8>, Vec<(u8, u64, u64)>);
    fn check(map: &BlockMap<u64, u64, u8>, holder: &[(u8, u64, u64)], count: usize);
    #[test]
    fn ascending();
    ///front-biased stream: the reversal tier flips the block (see
    ///blockset's `descending`)
    #[test]
    fn descending();
    #[test]
    fn stride();
    ///mixed-op torture mirroring blockset's: clustered + descending-bait
    ///streams, random removals, every holder name verified after every op
    #[test]
    fn mixed_ops_torture();
    ///appends stay dense; a mid insert into the full mask turns, the
    ///follow-up lands in a turn-minted hole (mirrors blockset's test)
    #[test]
    fn appends_dense_mid_insert_turns();
    ///fetch-distance banking (mirrors blockset's test): the 24 moved
    /// elements each emit ONE full-hop pair, then 3 banked inserts are
    /// no-ops
    #[test]
    fn gather_banks_holes_by_distance();
    ///front insert below cap: plain slide, both vecs move — 5 takes name 0
    #[test]
    fn front_insert_renames_min();
    ///tiny dense block, front insert: the ladder opens a slot at the back
    ///and slides the run into it — no rotation
    #[test]
    fn front_insert_back_opens();
    #[test]
    fn duplicates_rejected();
    ///insert leaves a pending (no-op) event — the next insert must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas();
    #[test]
    fn remove_and_reinsert();
    ///value/value_mut by key, and the values survive a reversal + turn
    #[test]
    fn value_lookup_and_mut();
    ///the full cursor battery (see blockset::assert_walks): both seeks ×
    ///both walk directions at every boundary, peek + addr, ghost
    ///round-trips. live = sorted live (key, value) pairs
    fn assert_walks<A: UnsignedNum>(map: &BlockMap<u64, u64, A>, live: &[(u64, u64)]);
    ///holed mask, forward block: cursor walks match the sorted model
    #[test]
    fn cursor_matches_sorted_model();
    ///flipped block: identical key-order semantics, values ride (see
    ///blockset's `cursor_flipped_block`)
    #[test]
    fn cursor_flipped_block();
    #[test]
    fn cursor_empty_and_single();
}
```
