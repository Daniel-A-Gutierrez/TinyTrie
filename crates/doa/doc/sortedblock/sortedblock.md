```rust
//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order (hole-adjacent mover
//!first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.
//!order: pos order == key order — normal (pos 0 = min) or reversed (pos 0
//!= max, Prepend bias), so a sequential insert at the key extreme is always
//!a physical tail push_back.
///L0024
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
///L0032
///key order along pos — Desc stores keys descending (pos 0 = max), so a
///sequential prepend is a tail push_back, never a slide
enum Direction {
    Asc,
    Desc,
}
///L0037
impl Direction {}
///L0061
///`a2p(a) = (a rol rotation) + offset`, `p2a` the exact inverse (both wrap in
///A). rotation turn = growth: every live pos maps to `2·pos − offset` mod A.
///offset turn = window move (heuristic tier, unimplemented).
#[derive(Clone, Copy)]
struct Translator<A> {
    rotation: u32,
    offset:   A,
}
///L0066
impl<A: UnsignedNum> Translator<A> {}
///L0098
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
///L0111
///expanded `(from, to)` pos pairs of one event, swap-cascade order
///(hole-adjacent mover first). `Run`: `p` cursor, `step` (−1 = run shifts
///up), `remaining` emissions; each pair is `(p, p − step)`.
enum DeltaPairs<'a> {
    Empty,
    Run { p: usize, step: i64, remaining: usize },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}
///L0117
impl Delta {}
///L0131
impl Iterator for DeltaPairs<'_> {}
///L0155
///slot moves of one mutation, compact event + translator snapshot at
///mutation time. `iter` expands to (from_addr, to_addr) name swaps
///applicable in order. per-call: `insert` panics while the previous call's
///deltas are pending — apply, then `clear`.
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}
///L0160
impl<A: UnsignedNum> Deltas<A> {}
///L0183
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
///L0194
impl<K, A> Drop for BlockSet<K, A> {}
///L0203
impl<K: Ord, A: UnsignedNum> BlockSet<K, A> {}
///L0519
#[cfg(test)]
mod tests {
    use super::*;
    ///apply translated deltas as name swaps, in the given order
    fn apply(holder: &mut Vec<(u8, u64)>, deltas: &Deltas<u8>);
    fn insert_all(
        keys: impl IntoIterator<Item = u64>,
        bias: InsertBias,
    ) -> (BlockSet<u64, u8>, Vec<(u8, u64)>);
    fn check(set: &BlockSet<u64, u8>, holder: &[(u8, u64)], count: usize);
    const BIASES: [InsertBias; 3] =
        [InsertBias::Insert, InsertBias::Append, InsertBias::Prepend];
    #[test]
    fn ascending();
    #[test]
    fn descending();
    #[test]
    fn stride();
    ///pure appends push_back instead of turning — cap tracks n, not the next
    ///power of two; the cadence holes sit BELOW the tail (appends can't
    ///consume them), so a spurious middle insert lands in one within budget
    ///— no turn
    #[test]
    fn append_bias_pushes_back_and_keeps_holes();
    ///reversed block: descending keys push_back at the tail — no front
    ///slides, cap tracks n (ascending keys hit pos 0 via the ascending test)
    #[test]
    fn prepend_bias_pushes_back_descending();
    ///front insert below cap: plain slide, name transfer without any turn —
    ///the run 20,10 cascades right (hole-adjacent first), 5 takes name 0
    #[test]
    fn front_insert_renames_min();
    ///tiny dense block, front insert: the tail run is within budget, so
    ///the ladder opens a slot at the back and slides the run into it —
    ///hole-adjacent cascade, no rotation (small caps never turn)
    #[test]
    fn front_insert_back_opens();
    #[test]
    fn duplicates_rejected();
    ///the compact NoneSlide event expands to the same cascade the old
    ///per-element pairs produced, both directions
    #[test]
    fn none_slide_expands_to_cascade();
    ///insert(2) leaves a pending (no-op) event — insert(3) must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas();
    ///the turn map (`a2p_new ∘ p2a_old`) is name-stable, order-preserving
    ///and in-bounds for every reachable translator — including the offsets
    ///grow_rotate currently refuses
    #[test]
    fn rotate_map_preserves_names_and_order();
    #[test]
    fn remove_and_reinsert();
}
```
