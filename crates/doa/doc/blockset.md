```rust
//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order for plain slides
//(hole-adjacent mover first; rebalances emit right-movers descending,
//!then left-movers ascending — see `DeltaPairs`) + a translator snapshot;
//!the holder applies `Deltas::iter`'s (from_addr, to_addr) name swaps in
//!order. far holes water-level their region instead of fetching (see
//!`rebalance_region`).
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme always lands at the physical tail
//!edge — push_back, or a direct place in a past-tail hole — never a
//!slide. the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): bidirectional key-ordered walk over raw
//!slots — invalidated by any mutation, re-seek.
//!keys live archived (serialize.rs): `K: Archive`, slots hold `K::Handle`
//!over an append-only side heap, reads yield `K::View`.
///L0032
///one fetch-gather slide-chain link: the hole moves `from → to`. Gather
///events speak these in post-prior-slides coordinates (see `Delta::Gather`)
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from: Pos,
    pub to:   Pos,
}
///L0037
impl NoneSlide {}
///L0046
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
    ///scratch, no per-bank alloc. blockmap's ladder only — blockset
    ///water-levels instead (see Rebalance)
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
///L0092
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
///L0122
///swap-cascade run state of one hole slide `from → to`: (first moved
///position, step ±1, count)
fn run_of(from: usize, to: usize) -> (usize, i64, usize);
///L0131
///the raw slide chain (see `Deltas::slides`)
pub(crate) enum Slides<'a> {
    Empty,
    One(NoneSlide),
    Many(std::slice::Iter<'a, NoneSlide>),
}
///L0137
impl Iterator for Slides<'_> {}
///L0154
///seed an expansion cursor from the event; None = nothing to expand
fn seed_state(event: &Delta, buf: &[NoneSlide]) -> Option<PairState>;
///L0179
///advance one expansion cursor; None = exhausted
fn step_state(state: &mut PairState, event: &Delta, buf: &[NoneSlide]) -> Option<(Pos, Pos)>;
///L0283
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
///L0296
impl<A: UnsignedNum> Deltas<A> {}
///L0418
///`Deltas::iter`'s return — pulls name pairs through the block-owned
///expansion, clearing at exhaust
pub struct DeltaIter<'a, A>(&'a mut Deltas<A>);
///L0420
impl<A: UnsignedNum> Iterator for DeltaIter<'_, A> {}
///L0441
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
///L0442
pub(crate) const TURN_DEN: usize = 6;
///L0445
///none-stride 2/(1−R) for R = 2/3 — the stride cap; not a power of two,
///the only divide left on it is the fetch count
pub(crate) const FREE_STRIDE: usize = 6;
///L0458
///water-leveling workability line (FREE_NUM/FREE_DEN): the region scan
///extends words outward while the scanned density is at or over 5/6 —
///a region is workable iff it fits a stride-≤ FREE_STRIDE lattice
///(live·FREE_DEN < span·FREE_NUM strictly; equality would need stride
///FREE_STRIDE+1), so the stop IS the stride cap. lower stops (the
///sketch's 3/4, then 7/8) are KNIFE-EDGED against the block's own
///steady state: a leveled area sits just above the stop and can never
///average DOWN to it — the scan ran to the nearest empty pocket
///(measured span-7522 rebalances). the BORROW rung is the hysteresis:
///rebalanced words lend until High, then one rebalance resets the area.
///see notes/session_history.md 2026-10-20
pub(crate) const FREE_NUM: usize = 5;
///L0459
pub(crate) const FREE_DEN: usize = 6;
///L0464
///the borrow line: a word lends holes while under 5/6 of capacity
///(53 of 64). borrows milk rebalanced words toward High before the
///next rebalance — ~5 lends per stride-4-lattice word per cycle
const HIGH: u32 = 53;
///L0469
///holes per borrow event: lendable guarantees ≥ 11 clear bits
///(popcount < HIGH), so one event banks 10 beside k — the old fetch's
///amortization bounded to the lending word
const BORROW_MIN: usize = 10;
///L0475
///the bank fork: a streak this hot banks (bulk-gather) instead of
///balancing or sliding — clusters amortize via banks, scattered
///inserts level via rebalances (balancing wins random, banking wins
///runs — measured both ways, session_history 2026-10-20)
const STREAK_BANK: usize = 2;
//only SkipSet's STRIDE stays a power of two (its slot math is
//shift/mask); the one divide left on FREE_STRIDE is the fetch count
///L0479
const _: () = assert!(FREE_STRIDE >= 2, "FREE_STRIDE must be ≥ 2");
///L0484
///z² for the reversal confidence gate — the low mean must clear this
///many standard deviations of the observed spread before flipping
///(2σ ≈ 95%)
const REVERSE_Z2: u128 = 4;
///L0491
///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;
///L0498
///turn census — turns are rare and O(cap), so timing them directly is
///overhead-free (clock reads amortize over microseconds of work). the
///bench reports the share of build time turns (grow_rotate: relocation
///loop + rotate_mask) consume. NOT API
#[doc(hidden)]
pub static TURN_COUNT: AtomicU64 = AtomicU64::new(0);
///L0500
#[doc(hidden)]
pub static TURN_MOVED: AtomicU64 = AtomicU64::new(0);
///L0502
#[doc(hidden)]
pub static TURN_TIME_NS: AtomicU64 = AtomicU64::new(0);
///L0508
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
///L0537
impl<K: Archive, A: UnsignedNum> BlockSet<K, A> {}
///L1871
///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K: Archive, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}
///L1876
impl<'a, K: Archive, A: UnsignedNum> Iterator for Iter<'a, K, A> {}
///L1894
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
///L1900
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L1907
impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {}
///L1968
#[cfg(test)]
mod tests {
    use super::*;
    ///apply translated deltas as name swaps, in the given order; returns
    ///the pair count (assert-and-apply in one pull — the drain IS the
    ///application)
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &mut Deltas<A>) -> usize;
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
    ///(a landing > FREE_STRIDE from the anchor) vs hot inserts
    ///(streak ≥ STREAK_BANK) per band, isolated on fresh prefills like
    ///the bench's independent rows. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_streak_census();
    ///one-off diagnostic: the exact anatomy of each streak break in the
    ///mid-band workloads — insert index, landing vs anchor, the event
    ///class that fired, and the anchor's true post-event position (a
    ///stale anchor names a false break). also logs turns/rebalances for
    ///correlation. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_break_forensics();
    ///the ceiling valve: a u8 block past the doubling midpoint, dense —
    ///a mid-block insert cannot turn, so the whole block water-levels
    ///onto the minimal-stride lattice that fits the remaining name
    ///space: 200 live, ceiling 256 → stride 5 (4/5 density), tail
    ///extended to the tight 251, and the lattice serves the following
    ///mid inserts cheaply instead of O(n) slides per insert
    #[test]
    fn ceiling_rebalances_whole_block();
    ///BORROW: k's word full, the right neighbor under High and holed —
    ///it lends its nearest hole across the boundary; no rebalance, no
    ///turn
    #[test]
    fn borrow_lends_across_words();
    ///FAR: both neighbor words at/over High — the region scan levels the
    ///block to a stride-4 lattice, k rides it at its boundary slot, no
    ///turn
    #[test]
    fn far_words_rebalance_region();
    ///the rebalance pair expansion is first-match-safe: lattice targets
    ///can coincide with other elements' sources (dense sub-regions), and
    ///the two-pass order (right-movers descending, left-movers ascending)
    ///must retarget each holder exactly once — verified by applying with
    ///first-match against hand-built region masks, at the boundary edges
    ///(at == 0, at == live, dense heads, holed tails)
    #[test]
    fn rebalance_pairs_first_match_safe();
    ///one-off diagnostic: the ladder's census over insertion patterns —
    ///slides (near + borrow + forced), rebalances (count, region span,
    ///moved), declined-path fetches, turns, total moved per insert.
    ///distinguishes by event shape: a slide's pairs == its single link
    ///span, a rebalance's pairs < it. run with --nocapture
    #[test]
    fn ladder_census();
    ///TAIL GROWTH: the region scan reaches the array end still above
    ///target — the block grows just enough to fit the lattice (nothing
    ///relocates to make room; fresh slots ARE the room), the rebalance
    ///levels including the last word, no turn
    #[test]
    fn tail_grows_to_target_density();
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
