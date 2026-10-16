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
///L0019
///one fetch-gather slide-chain link: the hole moves `from → to`. Gather
///events speak these in post-prior-slides coordinates (see `Delta::Gather`)
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from: Pos,
    pub to:   Pos,
}
///L0024
impl NoneSlide {}
///L0035
///one mutation event, compact. `Empty` = nothing pending (initial state;
///`clear` restores it — a no-op slide is still pending, `iter` just yields
///nothing)
enum Delta {
    Empty,
    ///the common contiguous run slide: the hole moves `from → to`, every
    ///element strictly between shifts one step toward `from`
    NoneSlide {
        from: Pos,
        to:   Pos,
    },
    ///a fetch-distance gather's slide chain, nearest hole first, each
    ///slide in post-prior-slides coordinates — identical to its pre-read
    ///position (a hole's slot is never touched before its own slide)
    Gather(Vec<NoneSlide>),
    ///rare non-run shapes (reversal, bulk)
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}
///L0058
///expanded `(from, to)` pos pairs of one event. plain slides emit 1-step
///cascades (hole-adjacent mover first); gathers walk the recorded
///slides' SEGMENTS and emit ONE pair per moved element carrying the
///FULL hop (crossing count) — no chained intermediate names. emission
///order keeps every `to` ahead of the remaining `from`s (first-match
///holders; overlapping hop pairs otherwise retarget twice)
enum DeltaPairs<'a> {
    Empty,
    ///one run's cursor: emits `(p, p − step)` per step
    Run { p: usize, step: i64, remaining: usize },
    Gather {
        slides: &'a [NoneSlide],
        right:  bool,
        ///walk counter over segments, FAR-segment first (segment index
        ///w−1−seg)
        seg:    usize,
        ///position cursor within the segment; `usize::MAX` = fresh
        q:      usize,
    },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}
///L0075
///swap-cascade run state of one hole slide `from → to`
fn run_of(from: usize, to: usize) -> (usize, i64, usize);
///L0083
impl Delta {}
///L0102
impl Iterator for DeltaPairs<'_> {}
///L0177
///slot moves of one mutation + the translator snapshot at mutation time.
///`iter` expands to (from_addr, to_addr) name swaps applicable in order;
///`insert` panics while deltas are pending — apply, then `clear`
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}
///L0182
impl<A: UnsignedNum> Deltas<A> {}
///L0233
///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
///preemptive turn line (numerator/denominator): past this density, a
///mid-block insert turns instead of gathering — past the line, the
///expected fetch distance blows through FREE_STRIDE (the stride banking
///assumption dies). 3/4 measured 2026-10-15: −18% random @10k but 2.25×
///WORSE @1M (turn frequency doubles; each big turn's O(cap) relocation
///eats the amortization) + 2× random-build memory — 7/8 stands
pub(crate) const TURN_NUM: usize = 7;
///L0234
pub(crate) const TURN_DEN: usize = 8;
///L0235
pub(crate) const FREE_STRIDE: usize = 8;
///L0242
///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;
///L0246
///mask covers data.len() slots; bit ⇔ slot initialized.
pub struct BlockSet<K, A> {
    translator: Translator<A>,
    data:       Vec<MaybeUninit<K>>,
    mask:       Mask,
    len:        usize,
    ///running sum of placed slot positions — the mean-insert-position
    ///signal for the reversal tier
    pos_sum:    u64,
}
///L0256
impl<K, A> Drop for BlockSet<K, A> {}
///L0265
impl<K: Ord, A: UnsignedNum> BlockSet<K, A> {}
///L0782
///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}
///L0787
impl<'a, K: Ord, A: UnsignedNum> Iterator for Iter<'a, K, A> {}
///L0801
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
    ///no-ops. the 24 moved elements each emit ONE full-hop pair (crossing
    ///count 4; farthest-first, descending — the first-match order)
    #[test]
    fn gather_banks_holes_by_distance();
    ///the LEFT-side gather mirror: holes below the spot, TWO non-empty
    ///segments — emission ascends (left walks up), far/low segment first,
    ///hop = crossing count per element
    #[test]
    fn gather_left_side_hop_pairs();
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
