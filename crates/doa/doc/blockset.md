```rust
//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order for plain slides
//(hole-adjacent mover first; rebalances emit right-movers descending,
//!then left-movers ascending — see `DeltaPairs`) + a translator snapshot;
//!the holder applies `Deltas::iter`'s (from_addr, to_addr) name swaps in
//!order. the boundary gap SUBDIVIDES first — k drops mid-gap, zero
//!movement; a nearest hole within FREE_STRIDE slides; farther holes
//!FORK on the locality streak — hot banks (concentrated strided
//!landings, ≥1 live per 16), cold water-levels (the scan absorbs space
//!outward, budget ≈ 2× the nearest hole, until the region's mean drops
//!to the block's and it is workable, then redistributes onto the
//!minimal-stride lattice, k at its rank); a solid mask mints tail
//!slots.
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme always lands at the physical tail
//!edge — push_back, or a direct place in a past-tail hole — never a
//!slide. the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): bidirectional key-ordered walk over raw
//!slots — invalidated by any mutation, re-seek.
//!keys live archived (serialize.rs): `K: Archive`, slots hold `K::Handle`
//!over an append-only side heap, reads yield `K::View`.
///L0038
///one fetch-gather slide-chain link: the hole moves `from → to`. Gather
///events speak these in post-prior-slides coordinates (see `Delta::Gather`)
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from: Pos,
    pub to:   Pos,
}
///L0043
impl NoneSlide {}
///L0052
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
    ///position (a hole's slot is never touched before its own slide).
    ///the slides live in `Deltas::buf[..n]` — the payload is reused
    ///scratch, no per-bank alloc. both tiers' banks record it
    Gather {
        n: usize,
    },
    ///a region's water-leveling rewrite: the region's live elements PLUS
    ///k (at rank `at` among them) redistribute onto the uniform
    ///one-hole-per-stride lattice `u(i) = i + i/(stride−1)` — old-rank i
    ///< at lands at u(i), i ≥ at at u(i+1); k takes u(at), a hole by
    ///construction. `mask` is the region's PRE-move occupancy (bit j ⇔
    ///region slot j live); pairs re-derive both endpoints from it
    Rebalance {
        base:   Pos,
        at:     usize,
        stride: usize,
        mask:    Mask,
    },
    ///bulk non-run shapes (unused; reversal reports no events by design)
    #[allow(dead_code)]
    Pairs(Vec<(Pos, Pos)>),
}
///L0097
///expanded `(from, to)` pos pairs of one event. plain slides emit 1-step
///cascades (hole-adjacent mover first); gathers walk the recorded
///slides' SEGMENTS and emit ONE pair per moved element carrying the
///FULL hop (crossing count) — no chained intermediate names. emission
///order keeps every `to` ahead of the remaining `from`s (first-match
///holders; overlapping hop pairs otherwise retarget twice). rebalances
///emit right-movers first in DESCENDING rank (every `to` sits above the
///remaining `from`s), then left-movers ascending (every `to` below them)
///— a target slot's old inhabitant, when live, moves the SAME direction
///and was already emitted (its rank is higher in the descending pass,
///lower in the ascending one), so first-match holders retarget exactly
///once
enum PairState {
    Idle,
    ///one run's cursor: emits `(p, p − step)` per step
    Run { p: usize, step: i64, remaining: usize },
    Gather {
        ///holes fetched from the right of the spot (else left) — sets the
        ///segment/hop direction
        right: bool,
        ///walk counter over segments, FAR-segment first (segment index
        ///w−1−seg)
        seg:   usize,
        ///position cursor within the segment; `usize::MAX` = fresh
        q:     usize,
    },
    ///the rebalance's two passes over the region mask: descending
    ///right-movers (`down_pos` walks `prev_set`), then ascending
    ///left-movers (`up_pos` walks `next_set`) — see the enum doc above
    Rebalance {
        rank:     usize,
        up_phase: bool,
        down_pos: usize,
        up_pos:   usize,
    },
    Pairs {
        idx: usize,
    },
}
///L0127
///swap-cascade run state of one hole slide `from → to`: (first moved
///position, step ±1, count)
fn run_of(from: usize, to: usize) -> (usize, i64, usize);
///L0136
///the raw slide chain (see `Deltas::slides`)
pub(crate) enum Slides<'a> {
    Empty,
    One(NoneSlide),
    Many(std::slice::Iter<'a, NoneSlide>),
}
///L0142
impl Iterator for Slides<'_> {}
///L0159
///seed an expansion cursor from the event; None = nothing to expand
fn seed_state(event: &Delta, buf: &[NoneSlide]) -> Option<PairState>;
///L0184
///advance one expansion cursor; None = exhausted
fn step_state(state: &mut PairState, event: &Delta, buf: &[NoneSlide]) -> Option<(Pos, Pos)>;
///L0307
///slot moves of one mutation + the translator snapshot at mutation time.
///the BLOCK owns its Deltas; consumers pull the expansion (`iter` /
///`next_pos`) — exhausting it clears the event, so apply loops need no
///`clear`; breaking early leaves it pending and the next insert panics.
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
    ///reused gather payload — `buf[..n]` while the event is Gather; the
    ///per-bank alloc dies
    buf:        Vec<NoneSlide>,
    ///the user's expansion cursor (see PairState)
    pair_state: PairState,
    ///bimap's chase cursor — a second pass over the same event, without
    ///consuming it (see next_chase)
    chase_state: PairState,
}
///L0320
impl<A: UnsignedNum> Deltas<A> {}
///L0446
///`Deltas::iter`'s return — pulls name pairs through the block-owned
///expansion, clearing at exhaust
pub struct DeltaIter<'a, A>(&'a mut Deltas<A>);
///L0448
impl<A: UnsignedNum> Iterator for DeltaIter<'_, A> {}
///L0469
///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
///preemptive turn line: past this density, a mid-block insert turns
///instead of gathering. 5/6 (2026-10-20): the ceiling = the scan stop —
///global density can never exceed workability, so a whole-block region
///search always terminates on its own (the growth panic's "impossible"
///invariant, now actually true), and the wide target-to-ceiling gap
///measured favorable. earlier lines: 7/8 (measured solid), 3/4 REJECTED
///twice (2026-10-15/19 — the loss is the LAYOUT: doubled cap → sparser
///probes + stretched deserts, random lookup 347→1132 at 1M)
pub(crate) const TURN_NUM: usize = 5;
///L0470
pub(crate) const TURN_DEN: usize = 6;
///L0473
///none-stride 2/(1−R) for R = 2/3 — the stride cap; not a power of two,
///the only divide left on it is the fetch count
pub(crate) const FREE_STRIDE: usize = 6;
//dead with the 2026-10-22 gather swap — the ladder subdivide/slides/
//banks/mints; no word lending, no streak banking
//const HIGH: u32 = 53;
//const BORROW_MIN: usize = 10;
///L0493
///the borrow line: a word lends holes while under 5/6 of capacity
///(53 of 64). borrows milk rebalanced words toward High before the
///next rebalance — ~5 lends per stride-4-lattice word per cycle
///holes per borrow event: lendable guarantees ≥ 11 clear bits
///(popcount < HIGH), so one event banks 10 beside k — the old fetch's
///amortization bounded to the lending word
///the bank fork: a streak this hot banks (concentrate) instead of
///balancing or sliding (spread) — clusters amortize via banks, scattered
///inserts level via rebalances (balancing wins random, banking wins
///runs — measured both ways, twice: the original ladder 2026-10-20, and
///the 2026-10-23 single-op sweep where each arm alone lost the other
///workload: bank-only random 572 vs 293, rebalance-only runs 5419 vs 68)
const STREAK_BANK: usize = 2;
///L0506
///water-leveling workability line (FREE_NUM/FREE_DEN): a region is
///workable iff it fits a stride-≤ FREE_STRIDE lattice
///(live·FREE_DEN < span·FREE_NUM strictly; equality would need stride
///FREE_STRIDE+1) — the workability FLOOR on the 2026-10-23 mean stop
///keeps planted lattice holes slide-reachable (a mean-only stop lets
///stride exceed the slide reach above 5/6 — measured grinding ~30
///pairs/insert at len 226/256). EXCEPTION: the whole-block stop
///(`!left && !right`, nothing left to absorb) can still stop
///unworkable — its lattice runs stride > FREE_STRIDE and the ladder
///re-levels instead of sliding; that state only arises when the turn
///is refused at the name ceiling with a holed mask
pub(crate) const FREE_NUM: usize = 5;
///L0507
pub(crate) const FREE_DEN: usize = 6;
//only SkipSet's STRIDE stays a power of two (its slot math is
//shift/mask); the one divide left on FREE_STRIDE is the fetch count
///L0511
const _: () = assert!(FREE_STRIDE >= 2, "FREE_STRIDE must be ≥ 2");
//z² variance gate — dead with the 2026-10-22 trigger swap (the
//reversal trigger is pos_sum < len·log2(len), no spread term)
//const REVERSE_Z2: u128 = 4;
///L0522
///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;
///L0529
///turn census — turns are rare and O(cap), so timing them directly is
///overhead-free (clock reads amortize over microseconds of work). the
///bench reports the share of build time turns (grow_rotate: relocation
///loop + rotate_mask) consume. NOT API
#[doc(hidden)]
pub static TURN_COUNT: AtomicU64 = AtomicU64::new(0);
///L0531
#[doc(hidden)]
pub static TURN_MOVED: AtomicU64 = AtomicU64::new(0);
///L0533
#[doc(hidden)]
pub static TURN_TIME_NS: AtomicU64 = AtomicU64::new(0);
///L0539
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
    //pos_sq_sum: u128 — the z-gate's variance half, dead with the
    //2026-10-22 trigger swap
    ///bumped by every position-relocating silent move (turns, flips) —
    ///slides/gathers don't bump (they record deltas). position holders
    ///(skipset tiers) use it as a staleness guard
    epoch:      u64,
    ///the previous insert's position — the locality streak's anchor,
    ///scaled like pos_sum across turns and flips
    last_insert: usize,
    ///consecutive inserts that landed within FREE_STRIDE of the
    ///previous one — the banking rungs' adaptive gather signal: a hot
    ///cluster banks deeper, so its next inserts find holes adjacent
    streak:     usize,
    ///the block's own event slot — the mutation feedback (see Deltas)
    deltas:     Deltas<A>,
}
///L0566
impl<K: Archive, A: UnsignedNum> BlockSet<K, A> {}
///L1921
///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K: Archive, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}
///L1926
impl<'a, K: Archive, A: UnsignedNum> Iterator for Iter<'a, K, A> {}
///L1944
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
///L1950
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L1957
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L2018
#[cfg(test)]
mod tests {
    use super::*;
    ///apply translated deltas as name swaps, in the given order; returns
    ///the pair count (assert-and-apply in one pull — the drain IS the
    ///application)
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &mut Deltas<A>) -> usize;
    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>);
    fn check<A: UnsignedNum>(set: &BlockSet<u64, A>, holder: &[(A, u64)], count: usize);
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
    ///the ceiling regime (cap == name space): a descending fill strands
    ///turn-minted holes past the push edge; the last inserts must BANK
    ///them in one gather (the len/2 gate's mint premise is void at the
    ///ceiling), not pay a same-length slide per remaining hole
    #[test]
    fn ceiling_banks_stranded_holes();
    ///one-off diagnostic: the anchor-follow check — the previous k's
    ///element rides the current insert's event, and `last_insert` must
    ///track it through every move site. for pair-recording events the
    ///event's own account is the oracle: apply the pos-pairs to the
    ///pre-insert anchor like a holder and compare against the element's
    ///true position. turn/flip moves are silent (no pairs) and verified
    ///internally instead — grow_rotate tracks exactly in its relocation
    ///loop, reverse mirrors exactly. run with --release --nocapture. NOT
    ///API
    #[test]
    fn streak_anchor_staleness();
    ///the hinted bracket must agree with the bsearch everywhere it
    ///answers: Some(Ok) = the same boundary position, Some(Err) = the
    ///same dup. holed blocks in both directions, probed at every key
    #[test]
    fn spot_near_agrees_with_bsearch();
    ///one-off diagnostic: the mid-band bench's streak health — resets
    ///(a landing > FREE_STRIDE from the anchor) vs hot inserts (a live
    ///streak) per band, isolated on fresh prefills like the bench's
    ///independent rows. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_streak_census();
    ///one-off diagnostic: the exact anatomy of each streak break in the
    ///mid-band workloads — insert index, landing vs anchor, the event
    ///class that fired, and the anchor's true post-event position (a
    ///stale anchor names a false break). also logs turns/rebalances for
    ///correlation. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_break_forensics();
    ///the ceiling regime (cap > the doubling midpoint, turns refused):
    ///a solid mask MINTS tail slots (want = 1 + dist/FREE_STRIDE,
    ///clamped to the name ceiling) and banks them — the 99-element run
    ///shifts once, and the banked holes absorb the following mid
    ///inserts with 1-pair slides instead of the O(n) grind
    #[test]
    fn ceiling_mints_tail_and_banks();
    ///BANK: the nearest hole 40 away — want = 40/FREE_STRIDE = 6 holes
    ///banked beside the boundary, the 40-element run shifts once, and
    ///the banked range absorbs the next insert with a 1-pair slide; no
    ///turn either time
    #[test]
    fn bank_absorbs_followers();
    ///FAR: the nearest hole 102 away (both flanking words solid) — a
    ///distance-proportional bank: want = 102/FREE_STRIDE = 17 holes
    ///COLD FAR: the nearest hole 102 away, streak dead — the
    ///water-leveling arm scans to the block mean (the emptied word 2
    ///drops it exactly there), redistributes onto the stride-4 lattice
    ///with k riding at its rank, no turn, no growth
    #[test]
    fn far_hole_water_levels();
    ///the rebalance pair expansion is first-match-safe: lattice targets
    ///can coincide with other elements' sources (dense sub-regions), and
    ///the two-pass order (right-movers descending, left-movers ascending)
    ///must retarget each holder exactly once — verified by applying with
    ///first-match against hand-built region masks, at the boundary edges
    ///(at == 0, at == live, dense heads, holed tails)
    #[test]
    fn rebalance_pairs_first_match_safe();
    ///STRIDED BANK, direct drive: compact_gather + record_gather with 49
    ///holes — a pinned unit test of the sentinel machinery, driven
    ///directly because the ladder only reaches drag ≥ 1 on a hot
    ///streak with dist ≥ 96. every
    ///aligned 16-window across the banked range holds ≥1 live (the skip
    ///list's stride window), and the pair expansion retargets every
    ///holder exactly once through the sentinel moves
    #[test]
    fn strided_bank_sentinels_direct();
    ///STRIDED BANK, LEFT mirror: the same 49-hole drive fetched from
    ///below — the sentinel landings mirror (open − (16i+15)), the run
    ///shifts down, and every aligned 16-window across the banked range
    ///still holds ≥1 live
    #[test]
    fn strided_bank_sentinels_left_direct();
    ///one-off diagnostic: the ladder's census over insertion patterns —
    ///slides (near + borrow + forced), rebalances (count, region span,
    ///moved), declined-path fetches, turns, total moved per insert.
    ///distinguishes by event shape: a slide's pairs == its single link
    ///span, a rebalance's pairs < it. run with --nocapture
    #[test]
    fn ladder_census();
    ///LEFT BANK: head holes 102 below the boundary — the bank fetches
    ///COLD LEFT: the nearest hole 102 below, streak dead — the scan
    ///absorbs the solid tail-side words, still unworkable, and GROWS
    ///the tail to fit the lattice (cap 200 → 228); k rides at its
    ///rank, no turn
    #[test]
    fn cold_left_grows_unworkable_tail();
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
    ///alternating front chunks (descending-in-Asc and ascending-in-Desc
    ///both land at the front) with tail filler keeping the all-time
    ///insert mean high — the mean trigger must hold the block steady
    ///instead of flip-flopping. NOTE: the z-gate that once gated this
    ///workload died with the 2026-10-23 trigger swap; this pins the
    ///mean side only — alternating front streams with NO tail filler
    ///are ungated (the mean never clears log2(len))
    #[test]
    fn reversal_trigger_holds_on_alternating_chunks();
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
