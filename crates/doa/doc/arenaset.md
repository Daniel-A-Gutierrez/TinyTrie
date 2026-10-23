```rust
//!ArenaSet: the block-of-blocks tier — many small BlockSets (u8 names,
//!cap 256) under one flat index, the shape the substrate was built for.
//!routing: a `BTreeMap<min-key clone, block id>` — the floor entry (the
//!greatest min ≤ k) owns k; k below every min routes to the first block
//!(a below-min insert, the only min-changing case — the index entry is
//!replaced). the index's order IS the block order: iteration walks its
//!values.
//!full blocks split: k beyond the tail extreme or below the min spawns
//!a FRESH block holding just k (no renames — k never landed); a mid-gap
//!k cleaves the full block 50/50 and ONE closed-form event reports it:
//!every element at position ≥ split_pos (mapped through from_tr) now
//!lives in the new block WITH THE SAME NAME (the halves copy as-is and
//!the translator's offset shift preserves names) — survivors and
//!boundary spawns rename nothing.
//!the per-block z-gate cannot fire on split-born halves (a dense
//!128-prefill needs len > 255 to trip — unreachable), so the ARENA
//!flips eagerly: a below-min k into a non-reversed block past warmup
//!reverses it first (one O(len) flip beats a per-insert front slide).
//!removal/merging: v0 (ids and names immortal, the arena never
//!shrinks). deltas: the touched block's own slot carries its name
//!swaps; `take_split` yields the cleave event — apply both before the
//!next insert.
///L0036
///the cleave event: every element of `from_aa` at position ≥ `split_pos`
///(mapped through `from_tr`) now lives in `to_aa` **with the same
///name** — the halves copy slots, heap, and translator as-is (the
///offset shift preserves names), so only the block id changes.
///elements below `split_pos` keep their names and block; boundary
///spawns record no split (k never landed in the old block)
pub struct Split {
    pub from_aa:   u32,
    pub from_tr:   Translator<u8>,
    pub to_aa:     u32,
    pub split_pos: usize,
}
///L0043
impl Split {}
///L0053
///block-of-blocks set over archived keys (see the module header)
pub struct ArenaSet<K: Archive> {
    ///the blocks; ids are Vec indices, immortal (v0: no removal, no reuse)
    arena:   Vec<BlockSet<K, u8>>,
    ///min-key clone → block id, one entry per block; the map order IS
    ///the block order
    index:   BTreeMap<K, u32>,
    len:     usize,
    ///the last-mutated block — its deltas slot holds the insert's event
    touched: u32,
    ///the last cleave event, until `take_split`
    split:   Option<Split>,
}
///L0066
impl<K: Archive + Clone + Ord> ArenaSet<K> {}
///L0260
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    ///the holder mirror's full contract per insert: the split predicate
    ///(old names of the source block), then the touched block's own
    ///event, then every held name resolves to its key. returns whether a
    ///split fired
    fn apply_events(holder: &mut Vec<((u32, u8), u64)>, arena: &mut ArenaSet<u64>) -> bool;
    fn assert_matches_model(arena: &ArenaSet<u64>, model: &BTreeSet<u64>);
    ///insert stream vs a BTreeSet model with a full holder mirror —
    ///every name verified after every insert, splits and block deltas
    ///applied exactly as a consumer would
    fn torture(keys: impl IntoIterator<Item = u64>);
    #[test]
    fn ascending_boundary_spawns();
    ///descending stream: below-min inserts flip fresh blocks (the z-gate
    ///past warmup) and full blocks spawn below — no cleaves either
    #[test]
    fn descending_spawns_below();
    #[test]
    fn matches_model_ascending();
    #[test]
    fn matches_model_descending();
    #[test]
    fn matches_model_random();
    ///mid-gap splits: a scattered stream with runs — full blocks cleave
    ///at least once, and the holder survives every predicate
    #[test]
    fn mid_cleaves_fire();
    ///string keys: the cleave's heap rebuild round-trips archived keys
    ///(materialize + re-archive into both halves' fresh compact heaps —
    ///handles are heap-relative, so the copied slots' handles die with
    ///the old heap)
    #[test]
    fn string_cleave_rebuilds_heaps();
    #[test]
    fn duplicates_rejected();
}
```
