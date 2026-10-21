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

use std::cmp::Ordering;
use std::mem::MaybeUninit;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use crate::index::*;
use crate::mask::{Mask, MaskCursor, SetBits, SetBitsRev};
use crate::serialize::Archive;
use crate::translator::Translator;

///one fetch-gather slide-chain link: the hole moves `from → to`. Gather
///events speak these in post-prior-slides coordinates (see `Delta::Gather`)
#[derive(Clone, Copy, Debug)]
pub struct NoneSlide {
    pub from: Pos,
    pub to:   Pos,
}

impl NoneSlide {
    pub(crate) fn new(from: Pos, to: Pos) -> Self {
        Self { from, to }
    }
}

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
    ///blockmap's ladder only — blockset water-levels instead (see
    ///Rebalance)
    Gather(Vec<NoneSlide>),
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
    ///the rebalance's two passes over the saved region mask: descending
    ///right-movers, then ascending left-movers (see the enum doc)
    Rebalance {
        down: SetBitsRev<'a>,
        up:   SetBits<'a>,
        ///region start — endpoints are region-local + base
        base: usize,
        ///k's rank — old-rank i shifts one up past it
        at:   usize,
        ///stride − 1: the lattice's live count per period
        s1:   usize,
        ///old-rank of the bit the active pass yields next
        rank: usize,
        ///the ascending pass started
        up_phase: bool,
    },
    Pairs(std::slice::Iter<'a, (Pos, Pos)>),
}

///swap-cascade run state of one hole slide `from → to`: (first moved
///position, step ±1, count)
fn run_of(from: usize, to: usize) -> (usize, i64, usize) {
    if from > to {
        (from - 1, -1, from - to)
    } else {
        (from + 1, 1, to - from)
    }
}

///the raw slide chain (see `Deltas::slides`)
pub(crate) enum Slides<'a> {
    Empty,
    One(NoneSlide),
    Many(std::slice::Iter<'a, NoneSlide>),
}

impl Iterator for Slides<'_> {
    type Item = NoneSlide;

    fn next(&mut self) -> Option<NoneSlide> {
        match self {
            Slides::Empty => None,
            Slides::One(s) => {
                let s = *s;
                *self = Slides::Empty;
                Some(s)
            }
            Slides::Many(it) => it.next().copied(),
        }
    }
}

impl Delta {
    fn pairs(&self) -> DeltaPairs<'_> {
        match self {
            Delta::Empty => DeltaPairs::Empty,
            Delta::NoneSlide { from, to } => {
                let (p, step, remaining) = run_of(from.0, to.0);
                DeltaPairs::Run { p, step, remaining }
            }
            Delta::Gather(v) => DeltaPairs::Gather {
                slides: v,
                right:  v.first().is_some_and(|s| s.from.0 > s.to.0),
                seg:    0,
                q:      usize::MAX,
            },
            Delta::Rebalance { base, at, stride, mask } => DeltaPairs::Rebalance {
                down: mask.iter_set_rev(),
                up:   mask.iter_set(),
                base: base.0,
                at:   *at,
                s1:   stride - 1,
                //the descending pass yields the HIGHEST bit first —
                //old-rank live−1
                rank: mask.count() - 1,
                up_phase: false,
            },
            Delta::Pairs(v) => DeltaPairs::Pairs(v.iter()),
        }
    }
}

impl Iterator for DeltaPairs<'_> {
    type Item = (Pos, Pos);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            DeltaPairs::Empty => None,
            DeltaPairs::Run { p, step, remaining } => {
                if *remaining == 0 {
                    return None;
                }
                *remaining -= 1;
                let q = *p;
                *p = (q as i64 + *step) as usize;
                Some((Pos(q), Pos((q as i64 - *step) as usize)))
            }
            DeltaPairs::Gather { slides, right, seg, q } => loop {
                let w = slides.len();
                if *seg >= w {
                    return None;
                }
                //segment (w−1−seg): far segments first, hop = w − segment
                //index — the crossing count (bounds in pre-event positions)
                let j = w - 1 - *seg;
                let (start, end, hop) = if *right {
                    let spot = slides[0].to.0;
                    if j == 0 {
                        (spot, slides[0].from.0, w as i64)
                    } else {
                        (
                            slides[j - 1].from.0 + 1,
                            slides[j].from.0,
                            (w - j) as i64,
                        )
                    }
                } else {
                    let spot = slides[0].to.0 + 1;
                    if j == 0 {
                        (slides[0].from.0 + 1, spot, -(w as i64))
                    } else {
                        (
                            slides[j].from.0 + 1,
                            slides[j - 1].from.0,
                            -((w - j) as i64),
                        )
                    }
                };
                //segment entry: walk away from the spot (right descends,
                //left ascends) so each `to` stays ahead of every remaining
                //`from` — first-match holders retarget once, never twice
                if *q == usize::MAX {
                    if end == start {
                        *seg += 1;
                        continue;
                    }
                    *q = if *right { end - 1 } else { start };
                }
                let from = *q;
                *q = if *right {
                    if from == start { usize::MAX } else { from - 1 }
                } else {
                    if from == end - 1 { usize::MAX } else { from + 1 }
                };
                if *q == usize::MAX {
                    *seg += 1;
                }
                return Some((Pos(from), Pos((from as i64 + hop) as usize)));
            },
            DeltaPairs::Rebalance { down, up, base, at, s1, rank, up_phase } => {
                let base = *base;
                if !*up_phase {
                    //pass 1: descending rank, right-movers only
                    while let Some(p) = down.next() {
                        let i = *rank;
                        //the last bit's decrement would underflow — the
                        //counter is dead once the pass ends
                        *rank = i.saturating_sub(1);
                        let j = i + usize::from(i >= *at);
                        let q = base + j + j / *s1;
                        if q > base + p {
                            return Some((Pos(base + p), Pos(q)));
                        }
                    }
                    *up_phase = true;
                    *rank = 0;
                }
                //pass 2: ascending rank, left-movers
                while let Some(p) = up.next() {
                    let i = *rank;
                    *rank = i + 1;
                    let j = i + usize::from(i >= *at);
                    let q = base + j + j / *s1;
                    if q < base + p {
                        return Some((Pos(base + p), Pos(q)));
                    }
                }
                None
            }
            DeltaPairs::Pairs(it) => it.next().copied(),
        }
    }
}

///slot moves of one mutation + the translator snapshot at mutation time.
///`iter` expands to (from_addr, to_addr) name swaps applicable in order;
///`insert` panics while deltas are pending — apply, then `clear`
pub struct Deltas<A> {
    event:      Delta,
    translator: Translator<A>,
}

impl<A: UnsignedNum> Deltas<A> {
    pub fn new() -> Self {
        Deltas { event: Delta::Empty, translator: Translator::new() }
    }

    ///(from_addr, to_addr) name swaps, applicable in order
    pub fn iter(&self) -> impl Iterator<Item = (A, A)> + '_ {
        self.event.pairs().map(|(f, t)| (self.translator.p2a(f), self.translator.p2a(t)))
    }

    ///the raw (from, to) POS pairs — no translation. valid only while
    ///the emitter's translator is unchanged since the record (immediate
    ///in-crate consumers — the flip now fires pre-record; `iter`'s names
    ///remain the era-stable contract for events held across mutations)
    pub(crate) fn iter_pos(&self) -> impl Iterator<Item = (Pos, Pos)> + '_ {
        self.event.pairs()
    }

    ///pending until `clear` — the caller contract
    pub(crate) fn is_pending(&self) -> bool {
        !matches!(self.event, Delta::Empty)
    }

    ///the raw slide chain — compact hole moves; the changed positions of
    ///each slide are [min(from, to), max(from, to)] inclusive. skipset's
    ///incremental upkeep reads these instead of the pair expansion
    pub(crate) fn slides(&self) -> Slides<'_> {
        match &self.event {
            Delta::Empty => Slides::Empty,
            Delta::NoneSlide { from, to } => Slides::One(NoneSlide::new(*from, *to)),
            Delta::Gather(v) => Slides::Many(v.iter()),
            //the whole region changed — one synthetic link spans it;
            //maintain only unions min/max, and re-reads are idempotent
            Delta::Rebalance { base, mask, .. } => {
                Slides::One(NoneSlide::new(*base, Pos(base.0 + mask.bits() - 1)))
            }
            Delta::Pairs(_) => Slides::Empty,
        }
    }

    ///reset to empty — call after applying
    pub fn clear(&mut self) {
        self.event = Delta::Empty;
        self.translator = Translator::new()
    }

    ///record a plain hole slide under the block's translator at record
    ///time (blockmap records the same events over its own slots)
    pub(crate) fn record_none_slide(&mut self, translator: Translator<A>, from: Pos, to: Pos) {
        self.translator = translator;
        self.event = Delta::NoneSlide { from, to };
    }

    ///record a fetch-gather's slide chain (blockmap's, same protocol)
    pub(crate) fn record_gather(&mut self, translator: Translator<A>, slides: Vec<NoneSlide>) {
        self.translator = translator;
        self.event = Delta::Gather(slides);
    }

    ///record a region rebalance (blockset's water-leveling rung). `mask`
    ///= the region's pre-move occupancy, `at` = k's rank in it (the
    ///lattice reserves u(at) for k), `stride` ≥ 2 (the lattice fit is
    ///rebalance_region's guarantee)
    pub(crate) fn record_rebalance(
        &mut self,
        translator: Translator<A>,
        base: Pos,
        at: usize,
        stride: usize,
        mask: Mask,
    ) {
        debug_assert!(stride >= 2);
        self.translator = translator;
        self.event = Delta::Rebalance { base, at, stride, mask };
    }
}

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
pub(crate) const TURN_DEN: usize = 6;
///none-stride 2/(1−R) for R = 2/3 — the stride cap; not a power of two,
///the only divide left on it is the fetch count
pub(crate) const FREE_STRIDE: usize = 6;

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
pub(crate) const FREE_DEN: usize = 6;

///the borrow line: a word lends holes while under 5/6 of capacity
///(53 of 64). borrows milk rebalanced words toward High before the
///next rebalance — ~5 lends per stride-4-lattice word per cycle
const HIGH: u32 = 53;

///holes per borrow event: lendable guarantees ≥ 11 clear bits
///(popcount < HIGH), so one event banks 10 beside k — the old fetch's
///amortization bounded to the lending word
const BORROW_MIN: usize = 10;

///the bank fork: a streak this hot banks (bulk-gather) instead of
///balancing or sliding — clusters amortize via banks, scattered
///inserts level via rebalances (balancing wins random, banking wins
///runs — measured both ways, session_history 2026-10-20)
const STREAK_BANK: usize = 2;

//only SkipSet's STRIDE stays a power of two (its slot math is
//shift/mask); the one divide left on FREE_STRIDE is the fetch count
const _: () = assert!(FREE_STRIDE >= 2, "FREE_STRIDE must be ≥ 2");

///z² for the reversal confidence gate — the low mean must clear this
///many standard deviations of the observed spread before flipping
///(2σ ≈ 95%)
const REVERSE_Z2: u128 = 4;

///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;

///turn census — turns are rare and O(cap), so timing them directly is
///overhead-free (clock reads amortize over microseconds of work). the
///bench reports the share of build time turns (grow_rotate: relocation
///loop + rotate_mask) consume. NOT API
#[doc(hidden)]
pub static TURN_COUNT: AtomicU64 = AtomicU64::new(0);
#[doc(hidden)]
pub static TURN_MOVED: AtomicU64 = AtomicU64::new(0);
#[doc(hidden)]
pub static TURN_TIME_NS: AtomicU64 = AtomicU64::new(0);


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
}

impl<K: Archive, A: UnsignedNum> BlockSet<K, A> {
    pub fn new() -> Self {
        BlockSet {
            translator: Translator::new(),
            data:      Vec::new(),
            heap:      Vec::new(),
            mask:      Mask::new(),
            len:       0,
            pos_sum:   0,
            pos_sq_sum: 0,
            epoch:     0,
            last_insert: 0,
            streak:     0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn cap(&self) -> usize {
        self.data.len()
    }

    pub fn max_capacity() -> usize {
        A::MAX.as_usize().saturating_add(1)
    }

    ///position epoch — bumped by turns/flips (silent position moves).
    ///equal epochs ⇒ positions observed earlier still hold
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    ///None if addr names no live element (never-minted names can't alias a
    ///live slot — a2p is injective; STALE pre-delta names can, see
    ///subtle_bugs §17)
    pub fn get<'s>(&'s self, addr: A) -> Option<K::View<'s>> {
        let p = self.translator.a2p(addr);
        (p.0 < self.data.len() && self.mask.get(p.0))
            .then(|| unsafe { K::view(self.data[p.0].assume_init_ref(), &self.heap) })
    }

    ///compare a stored handle against a query key — the tiers' compare
    pub fn cmp_key(&self, h: &K::Handle, k: &K) -> Ordering {
        K::cmp_key(h, k, &self.heap)
    }

    ///base position of a live addr — unchecked: a stale/dead addr still
    ///translates (a2p is total); the tiers pass only live addrs
    pub fn pos_of(&self, addr: A) -> usize {
        self.translator.a2p(addr).0
    }

    ///whether key order runs against pos order (Desc block)
    pub fn reversed(&self) -> bool {
        self.translator.reversed()
    }

    ///k beyond the tail extreme — the end a sequential insert extends.
    ///SkipSet::insert routes these to push_beyond, skipping its descent
    pub fn beyond_tail(&self, k: &K) -> bool {
        if self.len == 0 {
            return true;
        }
        let tail = self.key_tail();
        let ord = K::cmp_key(tail, k, &self.heap);
        if self.translator.reversed() {
            ord == Ordering::Greater
        } else {
            ord == Ordering::Less
        }
    }

    ///the reversal trigger — mean insert position below len/8 past the
    ///warmup, z-gated (see reverse_due). SkipSet drives reversal manually
    ///and reads this; its insert path never auto-flips (see insert_at)
    pub fn should_reverse(&self) -> bool {
        self.reverse_due()
    }

    ///the z-gated trigger: the mean gate (pos_sum·8 < len²) runs first —
    ///multiply-only, the hot path — and the variance test fires only
    ///when it already says front-biased. alternating streams (ascending
    ///chunks in a Desc block land at the front, and vice versa) crash the
    ///mean repeatedly with a huge spread — the z gate holds the block
    ///steady instead of thrashing; a tight front cluster (variance ~0)
    ///passes as before. multiply-only: (n²−8S)² > 64·z²·(Qn−S²) is
    ///(len/8 − μ)² > z²·σ² with the denominators cleared (n = len)
    fn reverse_due(&self) -> bool {
        if self.len <= 4 * FREE_STRIDE {
            return false;
        }
        let len = self.len as u64;
        if self.pos_sum.wrapping_mul(8) >= len.wrapping_mul(len) {
            return false;
        }
        let n = len as u128;
        let s = self.pos_sum as u128;
        let q = self.pos_sq_sum;
        let dev = n.saturating_mul(n).saturating_sub(8 * s); // ≥ 0: the gate held
        // Qn − S² ≥ 0 exactly — assuming no removals: the reflection's
        // n == len hardcode and the moment bookkeeping both drift under
        // remove; after removals + a flip the spread can go negative,
        // saturating to 0 and degrading the gate to the bare mean test
        let spread = q.saturating_mul(n).saturating_sub(s.saturating_mul(s));
        dev.saturating_mul(dev) > 64 * REVERSE_Z2 * spread
    }

    ///live handle at a base position — None if out of range or a hole.
    ///the tiers' incremental re-read
    pub fn slot(&self, pos: usize) -> Option<K::Handle> {
        (pos < self.data.len() && self.mask.get(pos)).then(|| unsafe { *self.key(pos) })
    }

    ///first live position ≥ from — None past the tail. the tiers'
    ///window-first sampling (tier0 slot j = the first item in window j,
    ///so banking/turns/bands can never empty a sample)
    pub fn first_live_pos(&self, from: usize) -> Option<usize> {
        self.mask.next_set(from)
    }

    ///live-run lengths between holes (position order) — the desert
    ///census: banking clusters holes and the solid runs it leaves
    ///behind are what the fetch walks. NOT API
    #[doc(hidden)]
    pub fn live_run_lengths(&self) -> Vec<usize> {
        let mut runs = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut prev = 0usize;
        for pos in self.mask.iter_set() {
            match run_start {
                None => run_start = Some(pos),
                Some(_) if pos == prev + 1 => {}
                Some(start) => {
                    runs.push(prev - start + 1);
                    run_start = Some(pos);
                }
            }
            prev = pos;
        }
        if let Some(start) = run_start {
            runs.push(prev - start + 1);
        }
        runs
    }

    ///bounded find within a position window [from, to): k's addr among
    ///live slots there, walked in key order from the window's key-min
    ///end. caller guarantees nothing — an empty window is None. the
    ///skipset tier jump; the walk covers the descent's final range
    pub fn find_window(&self, k: &K, from: usize, to: usize) -> Option<A> {
        let rev = self.translator.reversed();
        //key-min end of the window: pos `from` (Asc) / just below `to`
        //(Desc); the FIRST probe is window-bounded too — the loop's bound
        //checks only see later steps
        let mut p = if rev {
            if to == 0 {
                return None;
            }
            self.mask.prev_set(to)?
        } else {
            self.mask.next_set(from).filter(|probe| *probe < to)?
        };
        loop {
            // SAFETY: every step lands on a mask-named slot
            match unsafe { K::cmp_key(self.key(p), k, &self.heap) } {
                Ordering::Less => {}
                Ordering::Equal => return Some(self.translator.p2a(Pos(p))),
                Ordering::Greater => return None,
            }
            let q = if rev {
                let q = self.mask.prev_set(p)?;
                if q < from {
                    return None;
                }
                q
            } else {
                let q = self.mask.next_set(p + 1)?;
                if q >= to {
                    return None;
                }
                q
            };
            p = q;
        }
    }

    #[inline]
    pub fn find(&self, k: &K) -> Option<A> {
        //beyond either extreme → absent, no bsearch (see blockmap::find)
        if self.len > 0 {
            let tail = self.key_tail();
            let ord = K::cmp_key(tail, k, &self.heap);
            let beyond = if self.translator.reversed() { ord == Ordering::Greater } else { ord == Ordering::Less };
            if beyond {
                return None;
            }
            //len > 0 ⇒ a first live slot exists
            let head = unsafe { self.key(self.mask.next_set(0).unwrap()) };
            let ord = K::cmp_key(head, k, &self.heap);
            let before = if self.translator.reversed() { ord == Ordering::Less } else { ord == Ordering::Greater };
            if before {
                return None;
            }
        }
        match self.position(k) {
            Ok(p) => Some(self.translator.p2a(p)),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }

    ///first→last over live keys
    pub fn iter(&self) -> Iter<'_, K, A> {
        //direction picked once, never per element; Desc blocks: key order
        //ascends against pos order, so they walk positions down
        if self.translator.reversed() {
            Iter::Desc { bits: self.mask.iter_set_rev(), set: self }
        } else {
            Iter::Asc { bits: self.mask.iter_set(), set: self }
        }
    }

    ///cursor at the first key ≥ k, or the walk's end if none
    pub fn seek(&self, k: &K) -> Cursor<'_, K, A> {
        let mut cursor = Cursor::new(self);
        cursor.seek(k);
        cursor
    }

    ///cursor at the first key > k — k itself lands in a downward walk
    pub fn seek_after(&self, k: &K) -> Cursor<'_, K, A> {
        let mut cursor = Cursor::new(self);
        cursor.seek_after(k);
        cursor
    }

    ///first live slot at/after the pos boundary `from`, in KEY order —
    ///Desc runs key order against pos order, so its mirror is prev_set
    fn key_bound(&self, from: usize) -> Option<usize> {
        if self.translator.reversed() {
            self.mask.prev_set(from)
        } else {
            self.mask.next_set(from)
        }
    }

    ///deltas must be empty (applied + cleared) — panics if pending. Err
    ///returns k (dup, or out of names).
    pub fn insert(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.data.is_empty() {
            let h = K::archive(&k, &mut self.heap);
            return Ok(self.insert_first(h));
        }
        if self.len >= Self::max_capacity() {
            return Err(k);
        }
        //flip check BEFORE the record — no translator mutation happens
        //between the event record and insert's return, so pos-space
        //consumers (bimap's chase) read recorded coordinates directly
        self.maybe_reverse();
        let Some(spot) = self.spot(&k) else {
            return Err(k);
        };
        //archive after the dup check — a rejected key leaves no heap bytes
        let h = K::archive(&k, &mut self.heap);
        Ok(self.place_from(h, spot, deltas).0)
    }

    ///the ladder from a known spot: extend at the tail edge, open
    ///elsewhere. returns the addr with the placed position — callers
    ///holding positions skip the a2p round-trip (era-valid until the
    ///next silent move)
    fn place_from(&mut self, h: K::Handle, spot: Pos, deltas: &mut Deltas<A>) -> (A, Pos) {
        //sequential at the back: extend instead of sliding
        if spot.0 == self.data.len() && self.data.len() < Self::max_capacity() {
            (self.push_back(h, spot), spot)
        } else {
            let p = self.open(spot, deltas);
            (self.place(h, p), p)
        }
    }

    ///window-driven insert — SkipSet's tier descent provides the bounds.
    ///NEVER auto-reverses: the caller owns the flip (should_reverse +
    ///reverse), so caller-computed bounds can't be invalidated mid-call.
    ///
    /// # Safety
    ///
    /// `from ≤ to ≤ cap`, every live element before `from` in key order
    /// has key < k, and every element at/after `to` has key > k — k's
    /// boundary or a dup lies inside [from, to). the bsearch trusts the
    /// window; a wrong one corrupts the ordering. panics on pending
    /// deltas, like insert
    pub(crate) unsafe fn insert_at(
        &mut self,
        k: K,
        deltas: &mut Deltas<A>,
        from: usize,
        to: usize,
    ) -> Result<(A, Pos), K> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.data.is_empty() {
            let h = K::archive(&k, &mut self.heap);
            return Ok((self.insert_first(h), Pos(0)));
        }
        if self.len >= Self::max_capacity() {
            return Err(k);
        }
        match self.bsearch_bounds(&k, from, to) {
            Ok(_) => return Err(k), //dup
            Err(spot) => {
                let h = K::archive(&k, &mut self.heap);
                Ok(self.place_from(h, spot, deltas))
            }
        }
    }


    ///tail push with a caller guarantee: k is beyond the tail extreme
    ///(max Asc, min Desc — the end a sequential insert extends). no
    ///scan, no dup possible. SkipSet's beyond_tail pre-check routes
    ///appends here — one tail scan total instead of three
    pub(crate) fn push_beyond(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<(A, Pos), K> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.data.is_empty() {
            let h = K::archive(&k, &mut self.heap);
            return Ok((self.insert_first(h), Pos(0)));
        }
        if self.len >= Self::max_capacity() {
            return Err(k);
        }
        let spot = Pos(self.mask.last_set().unwrap() + 1);
        let h = K::archive(&k, &mut self.heap);
        Ok(self.place_from(h, spot, deltas))
    }

    ///in-place reversal: pos 0 = max, names follow the slots (the
    ///translator absorbs it — no deltas; a pending event stays valid).
    ///SkipSet calls this manually — plain insert auto-flips via
    ///maybe_reverse, insert_at never does
    pub(crate) fn reverse(&mut self) {
        self.data.reverse();
        self.mask.reverse();
        self.translator = self.translator.reverse(self.data.len());
        let len = self.len as u64;
        //reflect the bias signal around len — the trigger's own scale —
        // NOT the physical mirror (cap−1−p): cap imports density noise.
        // fires only when pos_sum < len²/8, so the subtraction never wraps
        let s = self.pos_sum as u128;
        let q = self.pos_sq_sum;
        self.pos_sum = len.wrapping_mul(len).wrapping_sub(self.pos_sum);
        //the reflection maps p → len − p, so Q' = n·len² − 2·len·S + Q
        //(n == len — the count is the scale)
        self.pos_sq_sum = (len as u128)
            .wrapping_mul(len as u128)
            .wrapping_mul(len as u128)
            .wrapping_sub((2 * len as u128).wrapping_mul(s))
            .wrapping_add(q);
        //positions mirrored — a silent move for pos holders; the streak
        //anchor reflects like the sums (clamped: a position past len
        //would wrap and just reset the streak, keep it meaningful)
        self.last_insert = (len as usize).saturating_sub(self.last_insert);
        self.epoch += 1;
    }

    ///flip a front-biased insert stream so future inserts land at the
    ///tail. fires when the mean sits below len/8 past the warmup,
    ///z-gated (see reverse_due). the check runs at insert's TOP — the
    ///trigger sees the pre-insert signal, and no translator mutation
    ///follows the event record inside insert (pos-space consumers rely
    ///on that)
    fn maybe_reverse(&mut self) {
        if self.reverse_due() {
            self.reverse();
        }
    }

    ///insertion spot, or None on dup
    fn spot(&self, k: &K) -> Option<Pos> {
        if self.len > 0 {
            //beyond the tail extreme (max Asc, min Desc — the end a
            //sequential insert extends) the spot is known without bsearch
            //— one last_set serves both the compare and the spot
            let p = self.mask.last_set().unwrap();
            // SAFETY: last_set names a live slot — mask ⇔ init
            let ord = K::cmp_key(unsafe { self.key(p) }, k, &self.heap);
            let beyond = if self.translator.reversed() { ord == Ordering::Greater } else { ord == Ordering::Less };
            if beyond {
                return Some(Pos(p + 1));
            }
        }
        self.bsearch(k).err()
    }

    ///vec-like first allocation: 4 slots, no translator turn until it fills
    fn insert_first(&mut self, h: K::Handle) -> A {
        self.data = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.mask = Mask::with_bits(4);
        self.place(h, Pos(0))
    }

    ///extend the allocation and place k at the edge — dense, holes never
    ///minted here. caller guarantees spot == cap < max_capacity
    fn push_back(&mut self, h: K::Handle, spot: Pos) -> A {
        self.push_slot();
        self.place(h, spot)
    }

    ///open a slot at/near `spot`: turns and gathers, escalating until a
    ///hole opens
    fn open(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let mut spot = spot;
        //APPEND FASTPATH: a solid tail run within a stride of the edge —
        //extend the block and shift the run up. ≤ FREE_STRIDE elements
        //move, no ladder rung, no banking; holed tails fall through (a
        //near hole within the stride slides even cheaper and grows
        //nothing), as does the spot-hole case (direct place)
        if spot.0 < self.cap()
            && self.cap() - spot.0 <= FREE_STRIDE
            && self.cap() < Self::max_capacity()
            && self.mask.get(spot.0)
            && (spot.0..self.cap()).all(|p| self.mask.get(p))
        {
            let from = self.cap();
            self.push_slot();
            return self.slide_open(from, spot.0, deltas);
        }
        //preemptive turn past the ceiling (TURN_NUM/DEN): one O(n)
        //turn beats the long scans (mid-block inserts only)
        if self.len.wrapping_mul(TURN_DEN) > self.data.len().wrapping_mul(TURN_NUM)
            && self.data.len() - spot.0 > FREE_STRIDE
        {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            }
        }
        self.gather_slots(spot, deltas)
    }

    ///k's spot under a completed turn: the old spot's name relocates via
    ///the turn's map (the same composition the relocation loop applies to
    ///live elements) — monotone, so the image is a valid insert boundary,
    ///no re-search
    fn turned_spot(&self, old: Translator<A>, spot: Pos) -> Pos {
        self.translator.a2p(old.p2a(spot))
    }

    ///write k at an open slot, mark live, mint its name; the slot position
    ///joins the running insert-position sum
    fn place(&mut self, h: K::Handle, p: Pos) -> A {
        self.data[p.0] = MaybeUninit::new(h);
        self.mask.set(p.0, true);
        self.len += 1;
        self.pos_sum = self.pos_sum.wrapping_add(p.0 as u64);
        self.pos_sq_sum = self
            .pos_sq_sum
            .wrapping_add((p.0 as u128) * (p.0 as u128));
        //locality streak: this insert within a stride of the previous
        self.streak =
            if p.0.abs_diff(self.last_insert) <= FREE_STRIDE { self.streak + 1 } else { 0 };
        self.last_insert = p.0;
        self.translator.p2a(p)
    }

    ///None if addr names no live element. the key is materialized back from
    ///the heap; its heap bytes leak (v1: no reclamation — see serialize.rs)
    pub fn remove(&mut self, addr: A) -> Option<K> {
        let pos = self.translator.a2p(addr);
        if pos.0 < self.data.len() && self.mask.get(pos.0) {
            self.mask.set(pos.0, false);
            self.len -= 1;
            let h = unsafe { *self.data[pos.0].assume_init_ref() };
            Some(K::materialize(h, &self.heap))
        } else {
            None
        }
    }

    ///bsearch over live slots — the raw boundary. Ok(p) = the
    ///duplicate at p; Err(spot) = a boundary in k's gap: all smaller
    ///elements before it, all greater at/after; may be a live slot or
    ///a hole (a hole spot is free space — gather exploits it)
    fn bsearch(&self, k: &K) -> Result<Pos, Pos> {
        //shape split at ~L2 residency: the branch lets the next probe's
        //load issue ahead of the compare (overlapping the miss — wins
        //DRAM-resident); the cmov bound has no mispredict (wins
        //cache-resident, where compares are cheap and a branch
        //mispredicts every probe)
        self.bsearch_bounds(k, 0, self.data.len())
    }

    ///bsearch over a position window [lo0, hi0) — insert_at's bounded
    ///search: the tier descent already narrowed the range, so this pays
    ///log2(window) probes instead of a linear walk (or the full-block
    ///log2(cap))
    fn bsearch_bounds(&self, k: &K, lo0: usize, hi0: usize) -> Result<Pos, Pos> {
        let big = self.data.len().wrapping_mul(std::mem::size_of::<K::Handle>()) > L2_BYTES;
        match (self.translator.reversed(), big) {
            (true, true) => self.search_spec::<true>(k, lo0, hi0),
            (true, false) => self.search_cmov::<true>(k, lo0, hi0),
            (false, true) => self.search_spec::<false>(k, lo0, hi0),
            (false, false) => self.search_cmov::<false>(k, lo0, hi0),
        }
    }

    ///speculating search — early exit, branch per probe. DRAM-resident
    ///blocks (see bsearch's split). the Equal early exit is LOAD-BEARING:
    ///gone, the two-way match compiles to cmov and the speculation (next
    ///probe's load issued ahead of the compare) dies — measured 1M lookup
    ///235→533
    fn search_spec<const REV: bool>(&self, k: &K, lo0: usize, hi0: usize) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (lo0, hi0);
        //holes skipped forward — everything live before the probe
        //compare known
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    //Desc: key order ascends against pos order — the
                    //compare flips (folded per instantiation)
                    let ord = K::cmp_key(probe, k, &self.heap);
                    let ord = if REV { ord.reverse() } else { ord };
                    match ord {
                        Ordering::Less => lo = p + 1,
                        Ordering::Greater => hi = p,
                        Ordering::Equal => return Ok(Pos(p)),
                    }
                }
            }
        }
        Err(Pos(lo))
    }

    ///fixed cmov search — no early exit, Equal narrows as Greater (the
    ///dup element stays in range), one final compare resolves it
    ///(cache-resident blocks; core's slice::binary_search)
    fn search_cmov<const REV: bool>(&self, k: &K, lo0: usize, hi0: usize) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (lo0, hi0);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    let ord = K::cmp_key(probe, k, &self.heap);
                    let ord = if REV { ord.reverse() } else { ord };
                    if ord == Ordering::Less {
                        lo = p + 1;
                    } else {
                        hi = p;
                    }
                }
            }
        }
        //final resolution: the boundary's first live element — the dup,
        //if the narrowing ended on or below it. equality is
        //direction-free
        if let Some(p) = self.next_live(lo, hi0) {
            if K::cmp_key(unsafe { self.key(p) }, k, &self.heap) == Ordering::Equal {
                return Ok(Pos(p));
            }
        }
        Err(Pos(lo))
    }

    ///locate k: Ok(p) = the duplicate at p; Err = the first LIVE
    ///element past k's run (bsearch's boundary mapped through
    ///`next_live`), or cap past the end
    fn position(&self, k: &K) -> Result<Pos, Pos> {
        match self.bsearch(k) {
            Ok(p) => Ok(p),
            Err(spot) => Err(self.next_live(spot.0, self.data.len()).map(Pos).unwrap_or(spot)),
        }
    }

    #[inline]
    fn next_live(&self, from: usize, bound: usize) -> Option<usize> {
        self.mask.next_set(from).filter(|p| *p < bound)
    }

    ///raw live-slot read
    ///
    /// # Safety
    ///
    /// p must name a live slot — mask ⇔ init
    unsafe fn key(&self, p: usize) -> &K::Handle {
        unsafe { self.data[p].assume_init_ref() }
    }

    ///live-slot view through the heap
    ///
    /// # Safety
    ///
    /// p must name a live slot — mask ⇔ init
    unsafe fn view_key(&self, p: usize) -> K::View<'_> {
        K::view(unsafe { self.key(p) }, &self.heap)
    }

    ///key at the physical tail (max Asc, min Desc); panics if len == 0
    fn key_tail(&self) -> &K::Handle {
        unsafe { self.key(self.mask.last_set().expect("live")) }
    }

    ///open a slot at/near `spot` — the ladder never fails: the region
    ///scan runs the whole block for space and the tail grows on demand
    ///(global density sits under the ratio, so space always exists);
    ///only the name ceiling is fatal. one `Delta` event per open. the
    ///rungs are WORD-CLASSIFIED (the old < FREE_STRIDE distance bound
    ///was a banking artifact):
    ///- NEAR: a hole in k's own word — one plain slide, or a bank when
    ///  the streak is hot and the hole is past a stride
    ///- BORROW: a neighboring word under the High line banks up to
    ///  BORROW_MIN (+ streak) holes beside k
    ///- FAR: the streak forks — cold balances (region rebalance + the
    ///  tail-growth valve), hot banks (the nearest hole within len/2,
    ///  else the growth-rebalance mints tail space)
    fn gather_slots(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        //k's slot already open (the bsearch converges on holes for
        //mid-gap inserts) — no fetch, no slide. nearest_clear excludes
        //`spot` by contract, so this is checked first
        if spot.0 < self.cap() && !self.mask.get(spot.0) {
            deltas.record_none_slide(self.translator, spot, spot);
            return spot;
        }
        //the nearest hole's distance picks the rung: within a stride one
        //plain slide; farther, the region water-levels instead of a
        //distance-proportitional fetch — banking dug deserts (the count-
        //bounded walk is quadratic in their depth); the rebalance
        //re-plants holes behind every element of the region
        let word_lo = spot.0 & !63;
        let up = self.mask.next_clear(spot.0 + 1);
        let down = spot.0.checked_sub(1).and_then(|b| self.mask.prev_clear(b));
        let nearest = |up: Option<usize>, down: Option<usize>| match (up, down) {
            (Some(u), Some(d)) => Some(if u - spot.0 <= spot.0 - d { u } else { d }),
            (u, d) => u.or(d),
        };
        //NEAR: a hole in k's own word — one plain slide, or a bank when
        //the streak is hot and the hole is further than a stride out (the
        //chunk frontier would otherwise re-pay the full run per insert)
        if let Some(hole) = nearest(up.filter(|h| *h < word_lo + 64), down.filter(|h| *h >= word_lo)) {
            if self.streak >= STREAK_BANK && hole.abs_diff(spot.0) > FREE_STRIDE {
                let right = hole > spot.0;
                let open = if right { spot.0 } else { spot.0 - 1 };
                let bound = if right { word_lo + 64 } else { word_lo };
                let holes = self.fetch_holes(spot, right, 1 + self.streak, bound);
                self.compact_gather(spot, open, right, &holes);
                deltas.record_gather(
                    self.translator,
                    holes
                        .iter()
                        .enumerate()
                        .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                        .collect(),
                );
                return Pos(open);
            }
            return self.slide_open(hole, spot.0, deltas);
        }
        //BORROW: a neighboring word under the High line lends — one
        //event banks up to BORROW_MIN holes beside k (the old fetch's
        //amortization, bounded to the lending word; a word milked from
        //3/4 lands at exactly High and stops lending). lendable implies
        //holes exist, so an eligible side's nearest hole is inside its
        //neighbor word (k's own word has none)
        let lend_left = word_lo > 0 && self.mask.word_at(word_lo - 64).count_ones() < HIGH;
        let lend_right = word_lo + 64 < self.cap() && self.mask.word_at(word_lo + 64).count_ones() < HIGH;
        if let Some(hole) = nearest(up.filter(|_| lend_right), down.filter(|_| lend_left)) {
            let right = hole > spot.0;
            let open = if right { spot.0 } else { spot.0 - 1 };
            let bound = if right { word_lo + 128 } else { word_lo - 64 };
            //a hot streak banks deeper — the cluster's next inserts land
            //right here, and the hole is further than a stride out
            let want = BORROW_MIN
                + usize::from(hole.abs_diff(spot.0) > FREE_STRIDE) * self.streak;
            let holes = self.fetch_holes(spot, right, want, bound);
            if holes.is_empty() {
                //a truncated lendable word with no holes: the single
                //nearest hole, however far
                return self.slide_open(hole, spot.0, deltas);
            }
            self.compact_gather(spot, open, right, &holes);
            deltas.record_gather(
                self.translator,
                holes
                    .iter()
                    .enumerate()
                    .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                    .collect(),
            );
            return Pos(open);
        }
        {
            //the fork: cold streaks balance, hot streaks bank
            if self.streak < STREAK_BANK {
                return self.rebalance_region(spot, deltas);
            }
            //hot: bank the nearest existing hole within len/2
            if let Some(hole) = self.mask.nearest_clear(spot.0)
                && hole.abs_diff(spot.0) <= self.len / 2
            {
                let right = hole > spot.0;
                let open = if right { spot.0 } else { spot.0 - 1 };
                //no region bound: right = no ceiling, left = no floor
                let bound = if right { usize::MAX } else { 0 };
                let want = 1 + hole.abs_diff(spot.0) / FREE_STRIDE
                    + usize::from(hole.abs_diff(spot.0) > FREE_STRIDE) * self.streak;
                let holes = self.fetch_holes(spot, right, want, bound);
                self.compact_gather(spot, open, right, &holes);
                deltas.record_gather(
                    self.translator,
                    holes
                        .iter()
                        .enumerate()
                        .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                        .collect(),
                );
                return Pos(open);
            }
            //past len/2 (or a full mask): the growth-rebalance mints
            //tail space — the bank's tail arm
            self.rebalance_region(spot, deltas)
        }
    }

    ///slide the hole to k's edge and record — near, borrow and forced
    ///share it. caller guarantees `hole` is the nearest on its side
    fn slide_open(&mut self, hole: usize, spot: usize, deltas: &mut Deltas<A>) -> Pos {
        let open = if hole > spot { spot } else { spot - 1 };
        self.slide_hole(hole, open);
        deltas.record_none_slide(self.translator, Pos(hole), Pos(open));
        Pos(open)
    }

    ///one plain slide: hole `from` → `to`
    fn slide_hole(&mut self, from: usize, to: usize) {
        unsafe {
            let ptr = self.data.as_mut_ptr();
            if from > to {
                std::ptr::copy(ptr.add(to), ptr.add(to + 1), from - to);
            } else {
                std::ptr::copy(ptr.add(from + 1), ptr.add(from), to - from);
            }
        }
        //the run between is fully live (nearest hole) — two-bit mask
        //update; the stale duplicate bytes ptr::copy leaves behind are
        //masked clear, never assume_init'd
        self.mask.set(from, true);
        self.mask.set(to, false);
    }

    ///one element relocation inside a rebalance pass. the pass orders
    ///make every target vacant when written (see rebalance_region); the
    ///stale source bytes are masked clear, never assume_init'd
    fn move_slot(&mut self, from: usize, to: usize) {
        self.data[to] = std::mem::replace(&mut self.data[from], MaybeUninit::uninit());
        self.mask.set(from, false);
        self.mask.set(to, true);
    }

    ///the `want` nearest holes on one side of `spot`, nearest-first
    ///(right: ascending, left: descending), within `bound` — the allowed
    ///region's far edge (right: exclusive ceiling, left: inclusive
    ///floor) — pre-read for `compact_gather`, no further scans. two
    ///callers: the borrow (the lending word's far edge) and the declined
    ///fetch (unbounded: MAX / 0)
    fn fetch_holes(&self, spot: Pos, right: bool, want: usize, bound: usize) -> Vec<usize> {
        let mut holes = Vec::with_capacity(want);
        if right {
            let mut cursor = spot.0;
            while holes.len() < want
                && let Some(h) = self.mask.next_clear(cursor)
            {
                if h >= bound {
                    break;
                }
                holes.push(h);
                cursor = h + 1;
            }
        } else {
            let mut cursor = spot.0 - 1;
            while holes.len() < want {
                let Some(h) = self.mask.prev_clear(cursor) else { break };
                if h < bound {
                    break;
                }
                holes.push(h);
                if h == 0 {
                    break;
                }
                cursor = h - 1;
            }
        }
        holes
    }

    ///gather compaction, one pass: the holes compact beside `spot`, every
    ///member Some crossing exactly once. k lands in `open`
    fn compact_gather(&mut self, spot: Pos, open: usize, right: bool, holes: &[usize]) {
        let w = holes.len();
        unsafe {
            let ptr = self.data.as_mut_ptr();
            //segment j (between consecutive holes) shifts by its crossing
            //count; far segments copy first so no live source is read
            //after being overwritten
            if right {
                //holes ascend; segment 0 spans [spot, h_0) — the run
                //that shifts up beside k's slot
                for j in (0..w).rev() {
                    let (lo, len) = if j == 0 {
                        (spot.0, holes[0] - spot.0)
                    } else {
                        (holes[j - 1] + 1, holes[j] - holes[j - 1] - 1)
                    };
                    let shift = w - j;
                    std::ptr::copy(ptr.add(lo), ptr.add(lo + shift), len);
                }
            } else {
                //holes descend; the top segment ends at spot − 1 (the
                //element at spot never moves — k lands below it)
                for k in (0..w).rev() {
                    let (lo, len) = if k == 0 {
                        (holes[0] + 1, spot.0 - 1 - holes[0])
                    } else {
                        (holes[k] + 1, holes[k - 1] - holes[k] - 1)
                    };
                    let shift = w - k;
                    std::ptr::copy(ptr.add(lo), ptr.add(lo - shift), len);
                }
            }
        }
        for &h in holes {
            self.mask.set(h, true);
        }
        //the vacated target range clears; sets first — a hole compacted
        //onto its own old position ends clear
        if right {
            for t in open..open + w {
                self.mask.set(t, false);
            }
        } else {
            for t in open + 1 - w..=open {
                self.mask.set(t, false);
            }
        }
    }

    ///water-leveling rung: scan words outward from `spot` until the
    ///scanned region's density drops under FREE_NUM/FREE_DEN, then
    ///redistribute the region's live elements PLUS k (at its rank
    ///boundary `r`) onto the uniform one-hole-per-stride lattice
    ///`u(i) = i + i/(stride−1)` — the minimal stride fitting live+1 in
    ///the region (≤ 8 at < 7/8 density). order-preserving; k's slot
    ///u(r) is a hole by construction, so the rebalance IS the open —
    ///the returned position is where the caller places k, and the ONE
    ///event covers everything. when the scan reaches the TAIL still
    ///above target, the block grows just enough to fit the lattice —
    ///fresh slots are the cheapest space. the scan never declines:
    ///global density sits under the ratio, so a whole-block search
    ///always finds space (or grows it); past the name ceiling (turns
    ///refused, §18) the nearest hole slides, however far
    fn rebalance_region(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let mut cap = self.data.len();
        //spot's word, clamped: spot == cap (dense tail) scans the last
        //word rather than reading past the range
        let mut wlo = spot.0.min(cap - 1) >> 6;
        let mut whi = wlo;
        let mut live = self.mask.word_at(wlo << 6).count_ones() as usize;
        let mut span = ((whi + 1) << 6).min(cap) - (wlo << 6);
        //extend word by word, sparser side first, until the region fits
        //a stride-≤ FREE_STRIDE lattice (strictly under FREE_NUM/FREE_DEN
        //— equality would need stride FREE_STRIDE+1)
        while live * FREE_DEN >= span * FREE_NUM {
            let left = wlo > 0;
            let right = (whi + 1) << 6 < cap;
            //the tail bounds the region and no free word lies left:
            //GROW it — fresh slots are the cheapest space (nothing
            //relocates, names mint on demand), ~1/7 of the region's live
            //at worst. this replaces the whole-block decline: the tail
            //word keeps refusing lends above High, so second-to-last
            //word inserts reach FAR and land here
            if !right && (!left || self.mask.word_at((wlo - 1) << 6).count_ones() > 0) {
                let need = live * FREE_DEN / FREE_NUM + 1;
                let grow = need - span;
                if cap + grow > Self::max_capacity() {
                    //the turn-refused regime (§18): growth is blocked,
                    //but the ladder's entry contract (len < ceiling)
                    //leaves room somewhere — a hole to slide, or one
                    //slot to mint (mask full implies cap == len <
                    //ceiling). the long slide is the only correct move
                    let hole = match self.mask.nearest_clear(spot.0) {
                        Some(hole) => hole,
                        None => {
                            self.push_slot();
                            self.cap() - 1
                        }
                    };
                    return self.slide_open(hole, spot.0, deltas);
                }
                self.push_slots(grow);
                cap = self.data.len();
                whi = (cap - 1) >> 6;
                span = cap - (wlo << 6);
                continue; //density now under target — the loop exits
            }
            let take_left = match (left, right) {
                (true, true) => {
                    let lw = self.mask.word_at((wlo - 1) << 6).count_ones();
                    let rw = self.mask.word_at((whi + 1) << 6).count_ones();
                    lw <= rw
                }
                (true, false) => true,
                _ => false,
            };
            if take_left {
                wlo -= 1;
                live += self.mask.word_at(wlo << 6).count_ones() as usize;
            } else {
                live += self.mask.word_at((whi + 1) << 6).count_ones() as usize;
                whi += 1;
            }
            span = ((whi + 1) << 6).min(cap) - (wlo << 6);
        }
        let base = wlo << 6;
        //clamped: the tail word may extend past cap, and the lattice's
        //positions must stay < cap
        let end = ((whi + 1) << 6).min(cap);
        if live == 0 {
            //an empty region (the holey tail word past the last live):
            //k takes the region's floor, no lattice needed
            deltas.record_none_slide(self.translator, Pos(base), Pos(base));
            return Pos(base);
        }
        let region = self.mask.copy_region(base, end);
        //k's rank among the region's live — the boundary is count-below
        //in the pre-move mask (order is preserved)
        let r = region.count_below((spot.0.min(end) - base).min(region.bits()));
        //minimal stride whose lattice holds live+1 (k included): u(live) < span
        let mut stride = 2;
        while live + live / (stride - 1) >= span {
            stride += 1;
        }
        debug_assert!(stride <= FREE_STRIDE, "density ≤ {FREE_NUM}/{FREE_DEN} bounds the stride");
        let s1 = stride - 1;
        //old-rank i's lattice slot: one past k from `r` up
        let target = |i: usize| i + usize::from(i >= r) + (i + usize::from(i >= r)) / s1 + base;
        debug_assert!(target(live - 1) < end, "the lattice must fit the region");
        //the two passes exactly as the pair emission (DeltaPairs::
        //Rebalance): right-movers descending — every target is vacant
        //when written (its old inhabitant, when live, is a right-mover
        //of higher rank, already moved) — then left-movers ascending,
        //the mirrored argument
        let mut i = live;
        for p in region.iter_set_rev() {
            i -= 1;
            let q = target(i);
            if q > base + p {
                self.move_slot(base + p, q);
            }
        }
        let mut i = 0;
        for p in region.iter_set() {
            let q = target(i);
            if q < base + p {
                self.move_slot(base + p, q);
            }
            i += 1;
        }
        deltas.record_rebalance(self.translator, Pos(base), r, stride, region);
        Pos(base + r + r / s1)
    }

    ///rotation turn (in-place): cap doubles, every live element relocates
    ///to `a2p_new(name)` — names preserved by construction. false ⇒
    ///refused (name-space midpoint passed, or rotation budget spent)
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.data.len();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        //refusal guards: doubled positions would exceed the address
        //space past the midpoint; the rotation knob would wrap
        if old_cap > A::MIDPOINT.as_usize()
            || self.translator.rotation() >= A::BIT_WIDTH as u32
        {
            return false;
        }
        let old_tr = self.translator;
        let turn_start = Instant::now();
        let new_tr = old_tr.rotate();
        self.data.reserve(old_cap);
        self.data.extend((old_cap..new_cap).map(|_| MaybeUninit::uninit()));
        //the map is monotone with each target ≥ its source — the
        //descending pass writes every target before its source is read.
        //any offset, either direction: the band (upper-half names land
        //odd) is rotate_mask's job
        for p in (0..old_cap).rev() {
            if self.mask.get(p) {
                let q = new_tr.a2p(self.translator.p2a(Pos(p)));
                debug_assert!(q.0 >= p, "turn must not move an element down");
                self.data[q.0] = std::mem::replace(&mut self.data[p], MaybeUninit::uninit());
            }
        }
        self.mask.rotate_mask(
            self.translator.offset().as_usize(),
            A::MIDPOINT.as_usize(),
            self.translator.reversed(),
        );
        self.translator = new_tr;
        //the pos-space doubled — the insert-position sum tracks it, and
        //squares quadruple
        self.pos_sum = self.pos_sum.wrapping_mul(2);
        self.pos_sq_sum = self.pos_sq_sum.wrapping_mul(4);
        //positions spread — a silent move for pos holders; the streak
        //anchor scales with them (approximate — the turn's band adds
        //one, like every stream signal here)
        self.last_insert = self.last_insert.wrapping_mul(2);
        self.epoch += 1;
        TURN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TURN_MOVED.fetch_add(old_cap as u64, std::sync::atomic::Ordering::Relaxed);
        TURN_TIME_NS.fetch_add(turn_start.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        true
    }

    ///extend by one uninit slot (mask clear) — the Vec's own doubling
    ///carries reallocation. panics at the name-space ceiling.
    fn push_slot(&mut self) {
        assert!(self.data.len() < Self::max_capacity(), "name space exhausted");
        self.data.push(MaybeUninit::uninit());
        self.mask.resize(self.data.len());
    }

    ///extend by `n` uninit slots (mask clear) in one resize — the
    ///rebalance's tail-growth valve. caller checks the name ceiling
    fn push_slots(&mut self, n: usize) {
        assert!(self.data.len() + n <= Self::max_capacity(), "name space exhausted");
        self.data.extend((0..n).map(|_| MaybeUninit::uninit()));
        self.mask.resize(self.data.len());
    }

    ///deferred — see CLAUDE.md Future Work; panics
    pub fn from_vec(v: Vec<K>) -> Self {
        let _ = v;
        todo!()
    }
}

///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K: Archive, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}

impl<'a, K: Archive, A: UnsignedNum> Iterator for Iter<'a, K, A> {
    type Item = K::View<'a>;

    #[inline]
    fn next(&mut self) -> Option<K::View<'a>> {
        match self {
            //mask ⇔ init invariant upholds the read
            Iter::Asc { bits, set } => bits.next().map(|p| unsafe { set.view_key(p) }),
            Iter::Desc { bits, set } => bits.next().map(|p| unsafe { set.view_key(p) }),
        }
    }
}

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

impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {
    fn new(set: &'a BlockSet<K, A>) -> Self {
        //dir: positions ascend — unless the block is flipped
        Cursor { set, walk: MaskCursor::start(&set.mask, !set.translator.reversed()) }
    }
}

impl<'a, K: Archive, A: UnsignedNum> Cursor<'a, K, A> {
    ///reposition at the first key ≥ k
    pub fn seek(&mut self, k: &K) {
        match self.set.bsearch(k) {
            Ok(p) => self.walk.at(p.0),
            Err(spot) => match self.set.key_bound(spot.0) {
                Some(q) => self.walk.at(q),
                None => self.walk.end(),
            },
        }
    }

    ///reposition at the first key > k (a dup at k skipped)
    pub fn seek_after(&mut self, k: &K) {
        let q = match self.set.bsearch(k) {
            //past the dup in key order — dir = Asc: forward skip
            Ok(p) => {
                if self.walk.dir() { self.set.mask.next_set(p.0 + 1) }
                else { self.set.mask.prev_set(p.0) }
            }
            Err(spot) => self.set.key_bound(spot.0),
        };
        if let Some(q) = q { self.walk.at(q) } else { self.walk.end() }
    }

    ///yield the current element, step forward (key-ascending). None at
    ///the walk's end.
    //#[inline(always)]: plain #[inline] left next outlined (both
    //directions' crossing scans inline into it, blowing LLVM's size
    //budget) and walk loops paid a call + cursor spill per step —
    ///measured 2× @10k fwd
    #[inline(always)]
    pub fn next(&mut self) -> Option<K::View<'a>> {
        let p = self.walk.slot()?;
        let set = self.set;
        let v = unsafe { set.view_key(p) };
        self.walk.advance(p);
        Some(v)
    }

    ///step back (key-descending), yield the element now current. None at
    ///the walk's start (the state stays at the first element — next
    ///re-yields it); from the forward ghost, resumes at the last element
    #[inline(always)]
    pub fn prev(&mut self) -> Option<K::View<'a>> {
        let q = self.walk.retreat()?;
        Some(unsafe { self.set.view_key(q) })
    }

    ///the element next() would yield, without moving
    pub fn key(&self) -> Option<K::View<'a>> {
        self.walk.slot().map(|p| unsafe { self.set.view_key(p) })
    }

    ///the current element's name
    pub fn addr(&self) -> Option<A> {
        self.walk.slot().map(|p| self.set.translator.p2a(Pos(p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///apply translated deltas as name swaps, in the given order
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &Deltas<A>) {
        for (from, to) in deltas.iter() {
            for (a, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
    }

    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>) {
        let mut set = BlockSet::new();
        let mut holder = Vec::new();
        let mut deltas = Deltas::new();
        for k in keys {
            match set.insert(k, &mut deltas) {
                Ok(addr) => {
                    apply(&mut holder, &deltas);
                    deltas.clear();
                    holder.push((addr, k));
                }
                Err(k) => panic!("insert of {k} failed"),
            }
        }
        (set, holder)
    }

    fn check(set: &BlockSet<u64, u8>, holder: &[(u8, u64)], count: usize) {
        assert_eq!(set.len(), count);
        assert_eq!(holder.len(), count);
        let mut prev = None;
        for k in set.iter() {
            if let Some(p) = prev {
                assert!(*k > p, "order broken");
            }
            prev = Some(*k);
        }
        for (a, k) in holder {
            assert_eq!(set.get(*a), Some(k), "holder name {a} -> key {k} broken");
        }
    }

    #[test]
    fn ascending() {
        let (s, h) = insert_all(0..200);
        check(&s, &h, 200);
    }

    ///front-biased stream: the mean-insert trigger flips the block past the
    ///warmup — the remaining inserts are tail pushes (they never reach the
    ///turn rung), names ride the flip, the holder mirror validates them
    #[test]
    fn descending() {
        let (s, h) = insert_all((0..200).rev());
        check(&s, &h, 200);
        assert!(s.translator.reversed(), "front bias must flip the block");
    }

    #[test]
    fn stride() {
        let (s, h) = insert_all((0..200).map(|i| (i * 37 + 11) % 211));
        check(&s, &h, 200);
    }

    ///mixed-op torture: clustered streams with interleaved descending bait
    ///(the flip) and random removals — gathers bank, turns fire under
    ///either direction, and every holder name is verified after every op
    #[test]
    fn mixed_ops_torture() {
        let mut set = BlockSet::<u64, u16>::new();
        let mut d = Deltas::new();
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
                //descending stretches bait the flip; clusters bait the gather
                let k = if i % 3 == 0 {
                    3000 - i * 5 + rnd() % 3
                } else {
                    (i / 40) * 700 + rnd() % 300
                };
                match set.insert(k, &mut d) {
                    Ok(a) => {
                        apply(&mut holder, &d);
                        d.clear();
                        holder.push((a, k));
                    }
                    Err(_) => d.clear(),
                }
            } else {
                let j = (rnd() % holder.len() as u64) as usize;
                set.remove(holder[j].0);
                holder.swap_remove(j);
            }
            for &(a, k) in &holder {
                assert_eq!(set.get(a), Some(&k), "op {i}: name {a} broken");
            }
        }
        let keys: Vec<u64> = set.iter().copied().collect();
        let asc = keys.windows(2).all(|w| w[0] < w[1]);
        let desc = keys.windows(2).all(|w| w[0] > w[1]);
        assert!(asc || desc, "order broken after the flip(s)");
        assert_eq!(keys.len(), holder.len());
    }

    ///appends stay dense — cap tracks n, no cadence holes; a mid insert
    ///into the full mask turns, and the follow-up mid insert lands in a
    ///turn-minted hole — no second turn
    #[test]
    fn appends_dense_mid_insert_turns() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in (0..100u64).map(|i| i * 2) {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        assert!(set.cap() < 128, "cap {} — turned instead of pushing back", set.cap());
        assert_eq!(set.cap() - set.len(), 0, "appends left holes");
        set.insert(101, &mut d).unwrap();
        d.clear();
        assert_eq!(set.cap(), 200, "mid insert into full mask must turn");
        set.insert(103, &mut d).unwrap();
        d.clear();
        assert_eq!(set.cap(), 200, "second mid insert must not turn");
        let keys: Vec<u64> = set.iter().copied().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(set.len(), 102);
    }

    ///BORROW: k's word full, the right neighbor under High and holed —
    ///it lends its nearest hole across the boundary; no rebalance, no
    ///turn
    #[test]
    fn borrow_lends_across_words() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        //words 0-1 dense (80 keys), then word 1's tail holed by removals
        for k in (0..80u64).map(|i| i * 2) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        //positions 66..79 clear — word 1 at 2/16 valid, well under
        //High; len 66 keeps the block under the 5/6 turn line
        for k in (66..80u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        let a = set.insert(51, &mut d).unwrap();
        //spot 26 (key 51 between 50@25 and 52@26): one event banks the
        //14 holes word 1 holds — the 40-element run [26, 66) shifts up,
        //k takes 26, holes land at 27..39
        assert_eq!(d.iter().count(), 40, "borrow bank pairs");
        assert_eq!(set.cap(), 80, "borrow must not turn");
        assert_eq!(set.get(26), Some(&51));
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 51));
        //len back under the 5/6 turn line, then the bank absorbs the
        //next insert: key 53 slides ONE element from a banked hole
        set.remove(set.find(&130).unwrap());
        holder.retain(|(_, hk)| *hk != 130);
        let a = set.insert(53, &mut d).unwrap();
        assert_eq!(set.cap(), 80, "no turn — the banked hole absorbs");
        assert_eq!(d.iter().count(), 1, "banked hole should absorb the insert");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 53));
        check(&set, &holder, 67);
    }

    ///FAR: both neighbor words at/over High — the region scan levels the
    ///block to a stride-4 lattice, k rides it at its boundary slot, no
    ///turn
    #[test]
    fn far_words_rebalance_region() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        //190 keys dense: words 0-1 full, word 2 holds 62 of 64 bits
        for k in (0..190u64).map(|i| i * 2) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        //word 2 emptied — the scan's ≤ 3/4 pocket
        for k in (128..190u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        //a far landing cools the streak, the removal restores len —
        //the probe must take the BALANCE arm of the fork (hot streaks
        //bank instead) and the len/2 bail stays dormant (len ≤ 128)
        let a = set.insert(261, &mut d).unwrap();
        assert_eq!(d.iter().count(), 0, "direct place, no movement");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 261));
        set.remove(a);
        holder.retain(|(_, hk)| *hk != 261);
        let a = set.insert(51, &mut d).unwrap();
        //spot 26 (key 51 between 50@25 and 52@26): k's word full, right
        //neighbor full (not borrowable) → FAR; region [0, 190), 128
        //live, density 67% < 5/6 → stride 4; k rides the lattice at
        //u(26) = 34; ranks 0..=2 keep their slots
        assert_eq!(d.iter().count(), 125, "rebalance pairs");
        assert_eq!(set.cap(), 190, "rebalance must not turn");
        assert_eq!(set.get(34), Some(&51), "k's reserved lattice slot");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 51));
        check(&set, &holder, 129);
        //uniform lattice: stride 4 ⇒ runs of ≤ 3
        assert!(set.live_run_lengths().into_iter().max().unwrap() <= 3, "lattice runs");
    }

    ///the rebalance pair expansion is first-match-safe: lattice targets
    ///can coincide with other elements' sources (dense sub-regions), and
    ///the two-pass order (right-movers descending, left-movers ascending)
    ///must retarget each holder exactly once — verified by applying with
    ///first-match against hand-built region masks, at the boundary edges
    ///(at == 0, at == live, dense heads, holed tails)
    #[test]
    fn rebalance_pairs_first_match_safe() {
        let cases: [(usize, usize, usize, usize, Vec<usize>); 6] = [
            (0, 0, 2, 64, (0..32).collect()),
            (10, 11, 4, 64, (0..45).collect()),
            (5, 20, 4, 128, (0..90).collect()),
            (0, 40, 5, 64, (0..48).collect()),
            (0, 45, 4, 64, (0..45).collect()),
            (64, 45, 2, 100, (0..35).chain(90..100).collect()),
        ];
        for (base, at, stride, bits, set) in cases {
            let mut mask = Mask::with_bits(bits);
            for p in &set {
                mask.set(*p, true);
            }
            let mut d = Deltas::<u8>::new();
            d.record_rebalance(Translator::new(), Pos(base), at, stride, mask);
            //holders: one per live element, named by its slot
            let mut holders: Vec<(u8, usize)> = set.iter().map(|p| ((base + p) as u8, *p)).collect();
            for (from, to) in d.iter() {
                for h in &mut holders {
                    if h.0 == from {
                        h.0 = to;
                        break;
                    }
                }
            }
            //expected: old-rank i lands at u(i + [i ≥ at])
            let s1 = stride - 1;
            let mut expected: Vec<(u8, usize)> = set
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let j = i + usize::from(i >= at);
                    ((base + j + j / s1) as u8, *p)
                })
                .collect();
            expected.sort_unstable();
            holders.sort_unstable();
            assert_eq!(holders, expected, "base {base} at {at} stride {stride}");
        }
    }

    ///one-off diagnostic: the ladder's census over insertion patterns —
    ///slides (near + borrow + forced), rebalances (count, region span,
    ///moved), declined-path fetches, turns, total moved per insert.
    ///distinguishes by event shape: a slide's pairs == its single link
    ///span, a rebalance's pairs < it. run with --nocapture
    #[test]
    fn ladder_census() {
        let report = |name: &str, keys: Vec<u64>| {
            let mut set = BlockSet::<u64, u32>::new();
            let mut d = Deltas::new();
            let (mut slides, mut s_moved, mut rebal, mut r_span, mut r_moved, mut gath, mut g_moved, mut turns) =
                (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
            let mut epoch = set.epoch();
            for k in keys {
                set.insert(k, &mut d).unwrap();
                if set.epoch() != epoch {
                    epoch = set.epoch();
                    turns += 1;
                }
                let pairs = d.iter().count() as u64;
                let links: Vec<_> = d.slides().collect();
                if links.is_empty() || links[0].from == links[0].to {
                    //push_back or a direct place — no movement
                } else if links.len() > 1 {
                    //banked event — borrow or declined fetch
                    gath += 1;
                    g_moved += pairs;
                } else {
                    let span = links[0].from.0.abs_diff(links[0].to.0) as u64;
                    if pairs == span {
                        //near / borrow / forced — one run slide
                        slides += 1;
                        s_moved += pairs;
                    } else {
                        rebal += 1;
                        r_span += span;
                        r_moved += pairs;
                    }
                }
                d.clear();
            }
            let n = set.len() as u64;
            println!(
                "{name:>8}: slides {slides} ({:.0}% of ins) moved {s_moved} | rebal {rebal} ({:.0}%) span {:.0} moved {:.0} | banked {gath} moved {g_moved} | turns {turns} | moved/ins {:.2}",
                slides * 100 / n,
                rebal * 100 / n,
                r_span / rebal.max(1),
                r_moved / rebal.max(1),
                (s_moved + r_moved + g_moved) / n,
            );
        };
        //scale via DOA_CENSUS_N (default 10k) — the runs-tiling
        //geometries diverge with scale
        let n: u64 = std::env::var("DOA_CENSUS_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);
        report("append", (0..n).collect());
        report("random", {
            let mut keys: Vec<u64> = (0..n).collect();
            let mut state = 0x2545_F491_4F6C_DD1Du64;
            for i in (1..keys.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                keys.swap(i, (state % (i as u64 + 1)) as usize);
            }
            keys
        });
        report("runs", {
            let mut chunks: Vec<Vec<u64>> = (0..n as usize / 100)
                .map(|chunk| ((chunk * 100) as u64..((chunk + 1) * 100) as u64).collect())
                .collect();
            for chunk in chunks.iter_mut().step_by(2) {
                chunk.reverse();
            }
            let mut state = 0x2545_F491_4F6C_DD1Du64;
            for i in (1..chunks.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunks.swap(i, (state % (i as u64 + 1)) as usize);
            }
            chunks.into_iter().flatten().collect()
        });
    }

    ///TAIL GROWTH: the region scan reaches the array end still above
    ///target — the block grows just enough to fit the lattice (nothing
    ///relocates to make room; fresh slots ARE the room), the rebalance
    ///levels including the last word, no turn
    #[test]
    fn tail_grows_to_target_density() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        //200 dense: word 3 = the 8-valid tail word; then holes at the
        //HEAD (global 75% ≤ 7/8, so no preemptive turn)
        for k in (0..200u64).map(|i| i * 2) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        for k in (0..50u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        //a far landing cools the streak — the probe must take the
        //BALANCE arm (hot streaks bank instead)
        let a = set.insert(1, &mut d).unwrap();
        assert_eq!(d.iter().count(), 0, "direct place, no movement");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 1));
        let epoch = set.epoch();
        let a = set.insert(301, &mut d).unwrap();
        //spot 151 (key 301 between 300@150 and 302@151): no hole at or
        //above the spot, the left word full → FAR; the scan takes the
        //sparse tail word,
        //hits the end, and grows 15 slots: region [128, 215), 72 live,
        //84% just under 5/6 → stride 6; k rides at u(23) = 155; ranks 0..=4 stay
        assert_eq!(set.cap(), 215, "the tail must grow by exactly need − span");
        assert_eq!(set.epoch(), epoch, "growth is silent-adjacent: no turn");
        assert_eq!(d.iter().count(), 67, "rebalance pairs");
        assert_eq!(set.get(155), Some(&301), "k's reserved lattice slot");
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 301));
        check(&set, &holder, 152);
    }

    ///front insert below cap: plain slide, name transfer without any turn —
    ///the run 20,10 cascades right (hole-adjacent first), 5 takes name 0
    #[test]
    fn front_insert_renames_min() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(10, &mut d).unwrap();
        d.clear();
        set.insert(20, &mut d).unwrap();
        d.clear();
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(2), Some(&20));
    }

    ///tiny dense block, front insert: the tail run is within budget, so
    ///the ladder opens a slot at the back and slides the run into it —
    ///hole-adjacent cascade, no rotation (small caps never turn)
    #[test]
    fn front_insert_back_opens() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in [10, 20, 30, 40] {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        let a = set.insert(5, &mut d).unwrap();
        assert_eq!(a, 0);
        assert_eq!(d.iter().collect::<Vec<_>>(), vec![(3, 4), (2, 3), (1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(4), Some(&40));
        assert_eq!(set.cap(), 5);
    }

    #[test]
    fn duplicates_rejected() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(1, &mut d).unwrap();
        assert!(set.insert(1, &mut d).is_err());
        assert_eq!(set.len(), 1);
    }

    ///the compact NoneSlide event expands to the same cascade the old
    ///per-element pairs produced, both directions
    #[test]
    fn none_slide_expands_to_cascade() {
        let expand = |from: usize, to: usize| -> Vec<(Pos, Pos)> {
            Delta::NoneSlide { from: Pos(from), to: Pos(to) }.pairs().collect()
        };
        assert_eq!(
            expand(10, 5),
            vec![
                (Pos(9), Pos(10)),
                (Pos(8), Pos(9)),
                (Pos(7), Pos(8)),
                (Pos(6), Pos(7)),
                (Pos(5), Pos(6))
            ],
            "run shifts up"
        );
        assert_eq!(
            expand(5, 9),
            vec![(Pos(6), Pos(5)), (Pos(7), Pos(6)), (Pos(8), Pos(7)), (Pos(9), Pos(8))],
            "run shifts down"
        );
        assert_eq!(expand(7, 7), Vec::<(Pos, Pos)>::new(), "from == to = no-op");
    }

    ///insert(2) leaves a pending (no-op) event — insert(3) must refuse it
    #[test]
    #[should_panic(expected = "pending deltas")]
    fn insert_panics_on_pending_deltas() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        set.insert(1, &mut d).unwrap();
        set.insert(2, &mut d).unwrap();
        set.insert(3, &mut d).unwrap();
    }

    ///the turn map (`a2p_new ∘ p2a_old`) is name-stable, order-preserving
    ///and in-bounds for every reachable translator — including the offsets
    ///grow_rotate currently refuses
    #[test]
    fn rotate_map_preserves_names_and_order() {
        for r in 0..u8::BIT_WIDTH as u32 {
            for o in [0u8, 1, 2, 63, 64, 127, 128, 200, 255] {
                let old = Translator::<u8>::with_params(r, o);
                let new = old.rotate();
                let mut prev = None;
                for p in 0..100usize {
                    let q = new.a2p(old.p2a(Pos(p)));
                    assert_eq!(new.p2a(q), old.p2a(Pos(p)), "names r={r} o={o} p={p}");
                    assert!(q.0 < 200, "bounds r={r} o={o} p={p}");
                    if let Some(pq) = prev {
                        assert!(q.0 > pq, "order r={r} o={o} p={p}");
                    }
                    prev = Some(q.0);
                }
            }
        }
    }

    ///z-gated trigger: alternating chunks (descending-in-Asc and
    ///ascending-in-Desc both land at the front) crash the mean from
    ///either direction — the variance gate must hold the block steady
    ///instead of flip-flopping every few hundred inserts
    #[test]
    fn reversal_z_gate_blocks_thrash() {
        let mut set = BlockSet::<u64, u16>::new();
        let mut d = Deltas::new();
        let mut flips = 0;
        let mut prev = false;
        for round in 0..30u64 {
            let base = round * 400;
            let keys: Vec<u64> = if round % 2 == 0 {
                (base..base + 100).collect()
            } else {
                (base..base + 100).rev().collect()
            };
            for k in keys {
                set.insert(k, &mut d).unwrap();
                d.clear();
                if set.translator.reversed() != prev {
                    prev = set.translator.reversed();
                    flips += 1;
                }
            }
            //tail filler keeps len growing and the mean bouncing — high
            //disjoint key range, so every filler lands at the tail
            for k in (base + 200..base + 400).map(|i| 1_000_000 + i * 3) {
                set.insert(k, &mut d).unwrap();
                d.clear();
                if set.translator.reversed() != prev {
                    prev = set.translator.reversed();
                    flips += 1;
                }
            }
        }
        assert!(flips <= 2, "the z gate must not thrash — {flips} flips");
    }

    #[test]
    fn remove_and_reinsert() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        for k in 0..50u64 {
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        let a = set.find(&25).unwrap();
        assert_eq!(set.remove(a), Some(25));
        assert_eq!(set.remove(a), None);
        set.insert(25, &mut d).unwrap();
        let mut keys: Vec<u64> = set.iter().copied().collect();
        keys.sort();
        assert_eq!(keys, (0..50).collect::<Vec<_>>());
    }

    ///the full model battery: both seeks × both walk directions at every
    ///boundary (below min, between, exact hits, above max), peek + addr,
    ///ghost round-trips. live = the sorted live keys
    fn assert_walks<A: UnsignedNum>(set: &BlockSet<u64, A>, live: &[u64]) {
        let max = live.last().unwrap() + 1;
        for k in 0..=max {
            let mut c = set.seek(&k);
            assert_eq!(c.key().copied(), live.iter().find(|&&x| x >= k).copied(), "peek {k}");
            if let (Some(x), Some(a)) = (c.key().copied(), c.addr()) {
                assert_eq!(set.find(&x), Some(a), "addr {k}");
            }
            let expect: Vec<u64> = live.iter().copied().filter(|x| *x >= k).collect();
            let got: Vec<u64> = std::iter::from_fn(|| c.next().copied()).collect();
            assert_eq!(got, expect, "seek({k}) forward");
            let mut c = set.seek(&k);
            let expect: Vec<u64> = live.iter().rev().copied().filter(|x| *x < k).collect();
            let got: Vec<u64> = std::iter::from_fn(|| c.prev().copied()).collect();
            assert_eq!(got, expect, "seek({k}) back");
            let mut c = set.seek_after(&k);
            let expect: Vec<u64> = live.iter().copied().filter(|x| *x > k).collect();
            let got: Vec<u64> = std::iter::from_fn(|| c.next().copied()).collect();
            assert_eq!(got, expect, "seek_after({k}) forward");
            let mut c = set.seek_after(&k);
            let expect: Vec<u64> = live.iter().rev().copied().filter(|x| *x <= k).collect();
            let got: Vec<u64> = std::iter::from_fn(|| c.prev().copied()).collect();
            assert_eq!(got, expect, "seek_after({k}) back");
        }
        //round-trip across the FwdEnd ghost; exhaustion is stable, both
        //sides resume forward correctly
        let mut c = set.seek(&live[0]);
        for k in live {
            assert_eq!(c.next(), Some(k), "roundtrip forward");
        }
        assert_eq!(c.next(), None);
        assert_eq!(c.next(), None, "FwdEnd must be stable");
        for k in live.iter().rev() {
            assert_eq!(c.prev(), Some(k), "roundtrip back");
        }
        assert_eq!(c.prev(), None);
        assert_eq!(c.prev(), None, "start must be stable");
        assert_eq!(c.next(), Some(&live[0]), "start resumes forward");
        //prev through the start then next again: the first element twice
        let mut c = set.seek(&live[0]);
        assert_eq!(c.prev(), None);
        assert_eq!(c.next(), Some(&live[0]));
        assert_eq!(c.next(), Some(&live[1]));
    }

    ///holed mask, forward block: walks match the sorted model. sized for
    ///miri — the battery sweeps every boundary regardless of n
    #[test]
    fn cursor_matches_sorted_model() {
        let mut set = BlockSet::<u64, u16>::new();
        let mut d = Deltas::new();
        //37 coprime 67 ⇒ distinct keys, 7 gaps for hole spots
        for i in 0..60u64 {
            let k = (i * 37 + 11) % 67;
            set.insert(k, &mut d).unwrap();
            d.clear();
        }
        for k in [11u64, 48, 18, 59, 66] {
            if let Some(a) = set.find(&k) {
                set.remove(a);
            }
        }
        let mut live: Vec<u64> = set.iter().copied().collect();
        live.sort_unstable();
        assert_walks(&set, &live);
    }

    ///flipped block: identical key-order semantics — the walk runs against
    ///pos order, names still match find
    #[test]
    fn cursor_flipped_block() {
        let (mut set, _) = insert_all((0..60u64).rev());
        assert!(set.translator.reversed(), "descending stream must flip");
        let removed = [3u64, 17, 42];
        for k in removed {
            set.remove(set.find(&k).unwrap());
        }
        let live: Vec<u64> = (0..60u64).filter(|k| !removed.contains(k)).collect();
        assert_walks(&set, &live);
    }

    #[test]
    fn cursor_empty_and_single() {
        let set = BlockSet::<u64, u8>::new();
        let mut c = set.seek(&5);
        assert_eq!(c.key(), None);
        assert_eq!(c.addr(), None);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), None);
        let mut set = set;
        let mut d = Deltas::new();
        set.insert(7, &mut d).unwrap();
        d.clear();
        let mut c = set.seek(&5);
        assert_eq!(c.next(), Some(&7));
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some(&7));
        assert_eq!(c.prev(), None);
        assert_eq!(c.next(), Some(&7));
        let mut c = set.seek_after(&7);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some(&7));
    }
}
