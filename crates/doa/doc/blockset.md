```rust
//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order for plain slides
//(hole-adjacent mover first; gathers emit full-hop pairs, far segment
//!first — see `DeltaPairs`) + a translator snapshot; the holder applies
//!`Deltas::iter`'s (from_addr, to_addr) name swaps in order.
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme always lands at the physical tail
//!edge — push_back, or a direct place in a past-tail hole — never a
//!slide. the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): bidirectional key-ordered walk over raw
//!slots — invalidated by any mutation, re-seek.
//!keys live archived (serialize.rs): `K: Archive`, slots hold `K::Handle`
//!over an append-only side heap, reads yield `K::View`.
///L0030
///one fetch-gather slide-chain link: the hole moves `from → to`. Gather
///events speak these in post-prior-slides coordinates (see `Delta::Gather`)
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from: Pos,
    pub to:   Pos,
}
///L0035
impl NoneSlide {}
///L0044
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
    ///bulk non-run shapes (unused; reversal reports no events by design)
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}
///L0067
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
        ///holes fetched from the right of the spot (else left) — sets the
        ///segment/hop direction
        right:  bool,
        ///walk counter over segments, FAR-segment first (segment index
        ///w−1−seg)
        seg:    usize,
        ///position cursor within the segment; `usize::MAX` = fresh
        q:      usize,
    },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}
///L0087
///swap-cascade run state of one hole slide `from → to`: (first moved
///position, step ±1, count)
fn run_of(from: usize, to: usize) -> (usize, i64, usize);
///L0096
///the raw slide chain (see `Deltas::slides`)
pub(crate) enum Slides<'a> {
    Empty,
    One(NoneSlide),
    Many(std::slice::Iter<'a, NoneSlide>),
}
///L0102
impl Iterator for Slides<'_> {}
///L0118
impl Delta {}
///L0137
impl Iterator for DeltaPairs<'_> {}
///L0212
///slot moves of one mutation + the translator snapshot at mutation time.
///`iter` expands to (from_addr, to_addr) name swaps applicable in order;
///`insert` panics while deltas are pending — apply, then `clear`
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}
///L0217
impl<A: UnsignedNum> Deltas<A> {}
///L0282
///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
///preemptive turn line: past this density, a mid-block insert turns
///instead of gathering — past the line the expected fetch distance
///blows through FREE_STRIDE. measured twice (2026-10-15, re-benched
///2026-10-19 with the turn census): 3/4 loses at 1M — turn TIME stays
///~1% either way, the loss is the LAYOUT (doubled cap → sparser bsearch
///probes + stretched banking deserts: random lookup 347→1132,
///moved/insert 13.7→68) while clustered runs DO win — a per-workload
///bet; revisit at the block-of-blocks tier
pub(crate) const TURN_NUM: usize = 7;
///L0283
pub(crate) const TURN_DEN: usize = 8;
///L0286
///gather banking distance: one hole fetched per FREE_STRIDE of distance
///to the nearest one
pub(crate) const FREE_STRIDE: usize = 8;
//FREE_STRIDE (and SkipSet's stride) stay powers of two — every divide and
//modulo on them compiles to shift/mask
///L0290
const _: () = assert!(FREE_STRIDE.count_ones() == 1, "FREE_STRIDE must be a power of 2");
///L0295
///z² for the reversal confidence gate — the low mean must clear this
///many standard deviations of the observed spread before flipping
///(2σ ≈ 95%)
const REVERSE_Z2: u128 = 4;
///L0302
///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;
///L0309
///turn census — turns are rare and O(cap), so timing them directly is
///overhead-free (clock reads amortize over microseconds of work). the
///bench reports the share of build time turns (grow_rotate: relocation
///loop + rotate_mask) consume. NOT API
#[doc(hidden)]
pub static TURN_COUNT: AtomicU64 = AtomicU64::new(0);
///L0311
#[doc(hidden)]
pub static TURN_MOVED: AtomicU64 = AtomicU64::new(0);
///L0313
#[doc(hidden)]
pub static TURN_TIME_NS: AtomicU64 = AtomicU64::new(0);
///L0319
///dense ordered set over archived keys: pos-ordered slot array holding
///`K::Handle` copies + a side heap of archived bytes.
///mask covers data.len() slots; bit ⇔ slot initialized.
pub struct BlockSet<K: Archive, A> {
    translator: Translator<A>,
    data:       Vec<MaybeUninit<K::Handle>>,
    ///K's archived heap bytes — append-only, unordered (see serialize.rs)
    heap:       Vec<u8>,
    mask:       Mask,
    len:        usize,
    ///running sum of placed slot positions — the mean-insert-position
    ///signal for the reversal tier
    pos_sum:    u64,
    ///running sum of squared positions — the variance half of the
    ///z-gated trigger; transformed like pos_sum (reflected on reversal,
    ///quadrupled on turns)
    pos_sq_sum: u128,
    ///bumped by every position-relocating silent move (turns, flips) —
    ///slides/gathers don't bump (they record deltas). position holders
    ///(skipset tiers) use it as a staleness guard
    epoch:      u64,
}
///L0339
impl<K: Archive, A: UnsignedNum> BlockSet<K, A> {}
///L1143
///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K: Archive, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}
///L1148
impl<'a, K: Archive, A: UnsignedNum> Iterator for Iter<'a, K, A> {}
///L1166
///key-ordered bidirectional walk — `seek`/`seek_after` position, `next`
///walks key-ascending (yield current, then step), `prev` walks back (step,
///then yield). stepping runs on `MaskCursor` (the cached-word walk, see
///mask.rs). holds raw slots: any mutation (slides, turns, flips)
///invalidates it — re-seek
pub struct Cursor<'a, K: Archive, A> {
    set:  &'a BlockSet<K, A>,
    ///the slot-walk machine; dir = !reversed, baked at construction
    walk: MaskCursor<'a>,
}
///L1172
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L1179
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L1240
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
    ///z-gated trigger: alternating chunks (descending-in-Asc and
    ///ascending-in-Desc both land at the front) crash the mean from
    ///either direction — the variance gate must hold the block steady
    ///instead of flip-flopping every few hundred inserts
    #[test]
    fn reversal_z_gate_blocks_thrash();
    #[test]
    fn remove_and_reinsert();
    ///the full model battery: both seeks × both walk directions at every
    ///boundary (below min, between, exact hits, above max), peek + addr,
    ///ghost round-trips. live = the sorted live keys
    fn assert_walks<A: UnsignedNum>(set: &BlockSet<u64, A>, live: &[u64]);
    ///holed mask, forward block: walks match the sorted model. sized for
    ///miri — the battery sweeps every boundary regardless of n
    #[test]
    fn cursor_matches_sorted_model();
    ///flipped block: identical key-order semantics — the walk runs against
    ///pos order, names still match find
    #[test]
    fn cursor_flipped_block();
    #[test]
    fn cursor_empty_and_single();
}
```
