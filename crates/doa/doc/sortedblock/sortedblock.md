```rust
//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order (hole-adjacent mover
//!first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme is always a physical tail
//!push_back. the translator carries the direction (see translator.rs).
///L0028
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
///L0043
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
///L0056
///swap-cascade run state of one hole slide `from → to`
fn run_of(from: usize, to: usize) -> (usize, i64, usize);
///L0064
impl Delta {}
///L0083
impl Iterator for DeltaPairs<'_> {}
///L0117
///slot moves of one mutation, compact event + translator snapshot at
///mutation time. `iter` expands to (from_addr, to_addr) name swaps
///applicable in order. per-call: `insert` panics while the previous call's
///deltas are pending — apply, then `clear`.
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}
///L0122
impl<A: UnsignedNum> Deltas<A> {}
///L0146
///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
const FREE_STRIDE: usize = 8;
///L0149
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
///L0160
impl<K, A> Drop for BlockSet<K, A> {}
///L0169
impl<K: Ord, A: UnsignedNum> BlockSet<K, A> {}
///L0585
#[cfg(test)]
mod tests {
    use super::*;
    ///apply translated deltas as name swaps, in the given order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &Deltas<A>);
    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>);
    fn check(set: &BlockSet<u64, u8>, holder: &[(u8, u64)], count: usize);
    #[test]
    fn ascending();
    ///front-biased stream: the mean-insert trigger flips the block past the
    ///warmup — the remaining inserts are tail pushes (they never reach the
    ///turn rung), names ride the flip, the holder mirror validates them
    #[test]
    fn descending();
    #[test]
    fn stride();
    ///mixed-op torture: clustered streams with interleaved descending bait
    ///(the flip) and random removals — gathers bank, turns fire under
    ///either direction, and every holder name is verified after every op
    #[test]
    fn mixed_ops_torture();
    ///appends stay dense — cap tracks n, no cadence holes; a mid insert
    ///into the full mask turns, and the follow-up mid insert lands in a
    ///turn-minted hole — no second turn
    #[test]
    fn appends_dense_mid_insert_turns();
    ///fetch-distance banking: holes 24 slots from the spot fetch 4 (one per
    ///FREE_STRIDE traveled) and land beside k — the next 3 inserts are
    ///no-ops. the holder rides the 96-pair chain (4 slides × 24-slot runs)
    #[test]
    fn gather_banks_holes_by_distance();
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
