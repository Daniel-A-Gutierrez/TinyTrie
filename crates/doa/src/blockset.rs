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
//!keys live archived (v2, crates/block): slots hold `K::Handle` copies
//!over the set's OWN `Block<P>` — the whole tier is ONE serialized
//!allocation. the block's stack carries the occupancy words (fixed region,
//!preallocated for the slot ceiling, pushed first — it can never grow) and
//!the Handle slots (the stack's LAST item, growing by `extend_buf`); the
//!heap bytes hang off K::archive through the block's Allocator. reads use
//!`RelOrd::rel_cmp` over `Heap::new(&self)` — a `&Block` view, so find/
//!bsearch stay `&self` throughout. capacity feedback is try-then-rewind:
//!`Allocator::snapshot` → `K::archive` returns Ok (committed) or Err →
//!`rewind` (heap wiped to the snapshot — the block's zero-record
//!invariant), and the insert fails atomically with both fronts unchanged.
//!removal/replace still leak heap bytes (v0 — release wiring is deferred
//!with the freelist; the deltas/holder protocols don't model freed
//!regions yet).

use std::cmp::Ordering;
use std::marker::PhantomData;
use std::mem::{size_of, MaybeUninit};
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use block::{Archive, Block, H, Heap, MAX_ALIGN, Ptr, RelOrd, S};

///the byte ceiling one block can hold — aligned DOWN, since block.grow
///rounds its target UP to MAX_ALIGN before the width check
fn pmax<P: Ptr>() -> usize {
    P::MAX.as_usize().min(u32::MAX as usize) & !(MAX_ALIGN - 1)
}

use crate::index::*;
use crate::mask::Mask;
use crate::translator::Translator;

///default slot ceiling of `new()`: a cache-resident-ish block (~1 MB of
///u64-key data + its mask). with_capacity names it explicitly
const DEFAULT_SLOTS: usize = 1 << 17;

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
#[allow(dead_code)] //skipset's consumer is legacy-gated; blockset's tests exercise it
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

///seed an expansion cursor from the event; None = nothing to expand
fn seed_state(event: &Delta, buf: &[NoneSlide]) -> Option<PairState> {
    Some(match event {
        Delta::Empty => return None,
        Delta::NoneSlide { from, to } => {
            let (p, step, remaining) = run_of(from.0, to.0);
            PairState::Run { p, step, remaining }
        }
        Delta::Gather { n } => PairState::Gather {
            right: buf[..*n].first().is_some_and(|s| s.from.0 > s.to.0),
            seg:   0,
            q:     usize::MAX,
        },
        Delta::Rebalance { mask, .. } => PairState::Rebalance {
            //the descending pass yields the HIGHEST bit first —
            //old-rank live−1
            rank:     mask.count() - 1,
            up_phase: false,
            down_pos: mask.bits(),
            up_pos:   0,
        },
        Delta::Pairs(_) => PairState::Pairs { idx: 0 },
    })
}

///advance one expansion cursor; None = exhausted
fn step_state(state: &mut PairState, event: &Delta, buf: &[NoneSlide]) -> Option<(Pos, Pos)> {
    match (state, event) {
        (PairState::Run { p, step, remaining }, _) => {
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            let q = *p;
            *p = (q as i64 + *step) as usize;
            Some((Pos(q), Pos((q as i64 - *step) as usize)))
        }
        (PairState::Gather { right, seg, q }, Delta::Gather { n }) => loop {
            let w = *n;
            //dragged sentinels: one per 15 holes — the strided landings'
            //gaps hold the run's head elements (see record_gather)
            let drag = (w - 1) / 15;
            if *seg >= w {
                //sentinel pairs, DESCENDING — every target sits above the
                //remaining sentinel sources and below the contiguous
                //block's pairs (emitted earlier)
                if drag == 0 || *q == 0 {
                    return None;
                }
                if *q == usize::MAX {
                    *q = drag;
                }
                *q -= 1;
                let (i, open) = (*q, buf[0].to.0);
                return Some(if *right {
                    (Pos(open + i), Pos(open + 16 * i + 15))
                } else {
                    (Pos(open - i), Pos(open - 16 * i - 15))
                });
            }
            //segment (w−1−seg): far segments first, hop = w − segment
            //index — the crossing count (bounds in pre-event positions).
            //seg-0 excludes its head `drag` elements — they're sentinels
            let j = w - 1 - *seg;
            let (start, end, hop) = if *right {
                let spot = buf[0].to.0;
                if j == 0 {
                    (spot + drag, buf[0].from.0, w as i64)
                } else {
                    (buf[j - 1].from.0 + 1, buf[j].from.0, (w - j) as i64)
                }
            } else {
                let spot = buf[0].to.0 + 1;
                if j == 0 {
                    (buf[0].from.0 + 1, spot - drag, -(w as i64))
                } else {
                    (buf[j].from.0 + 1, buf[j - 1].from.0, -((w - j) as i64))
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
        (
            PairState::Rebalance { rank, up_phase, down_pos, up_pos },
            Delta::Rebalance { base, at, stride, mask },
        ) => {
            let (base, at, s1) = (base.0, *at, stride - 1);
            if !*up_phase {
                //pass 1: descending rank, right-movers only
                while let Some(p) = mask.prev_set(*down_pos) {
                    *down_pos = p;
                    let i = *rank;
                    //the last bit's decrement would underflow — the
                    //counter is dead once the pass ends
                    *rank = i.saturating_sub(1);
                    let j = i + usize::from(i >= at);
                    let q = base + j + j / s1;
                    if q > base + p {
                        return Some((Pos(base + p), Pos(q)));
                    }
                }
                *up_phase = true;
                *rank = 0;
            }
            //pass 2: ascending rank, left-movers
            while let Some(p) = mask.next_set(*up_pos) {
                *up_pos = p + 1;
                let i = *rank;
                *rank = i + 1;
                let j = i + usize::from(i >= at);
                let q = base + j + j / s1;
                if q < base + p {
                    return Some((Pos(base + p), Pos(q)));
                }
            }
            None
        }
        (PairState::Pairs { idx }, Delta::Pairs(v)) => {
            let p = v.get(*idx).copied()?;
            *idx += 1;
            Some(p)
        }
        //state/event mismatch — unreachable (states seed from events)
        _ => None,
    }
}

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

impl<A: UnsignedNum> Deltas<A> {
    pub fn new() -> Self {
        Deltas {
            event:       Delta::Empty,
            translator:  Translator::new(),
            buf:         Vec::new(),
            pair_state:  PairState::Idle,
            chase_state: PairState::Idle,
        }
    }

    ///pending until exhausted or `clear`
    pub fn is_pending(&self) -> bool {
        !matches!(self.event, Delta::Empty)
    }

    ///(from_addr, to_addr) name swaps, applicable in order; exhausting
    ///clears the event
    pub fn iter(&mut self) -> DeltaIter<'_, A> {
        DeltaIter(self)
    }

    ///one expanded (from, to) POS pair, or None (the event is exhausted —
    ///cleared). pub(crate): the pos-space consumers (bimap's chase)
    ///pull directly; name-space holders use `iter`
    pub(crate) fn next_pos(&mut self) -> Option<(Pos, Pos)> {
        if matches!(self.pair_state, PairState::Idle) {
            self.pair_state = seed_state(&self.event, &self.buf)?;
        }
        let pair = step_state(&mut self.pair_state, &self.event, &self.buf);
        if pair.is_none() {
            self.event = Delta::Empty;
            self.pair_state = PairState::Idle;
        }
        pair
    }

    ///bimap's chase pass: pulls the full expansion through its OWN cursor
    ///without clearing or touching the user's — the event stays pending
    ///for the holder to drain through `iter`
    pub(crate) fn next_chase(&mut self) -> Option<(Pos, Pos)> {
        if matches!(self.chase_state, PairState::Idle) {
            self.chase_state = seed_state(&self.event, &self.buf)?;
        }
        step_state(&mut self.chase_state, &self.event, &self.buf)
    }

    ///the raw slide chain — compact hole moves; the changed positions of
    ///each slide are [min(from, to), max(from, to)] inclusive. skipset's
    ///incremental upkeep reads these instead of the pair expansion
    #[allow(dead_code)]
    pub(crate) fn slides(&self) -> Slides<'_> {
        match &self.event {
            Delta::Empty => Slides::Empty,
            Delta::NoneSlide { from, to } => Slides::One(NoneSlide::new(*from, *to)),
            Delta::Gather { n } => Slides::Many(self.buf[..*n].iter()),
            //the whole region changed — one synthetic link spans it;
            //maintain only unions min/max, and re-reads are idempotent
            Delta::Rebalance { base, mask, .. } => {
                Slides::One(NoneSlide::new(*base, Pos(base.0 + mask.bits() - 1)))
            }
            Delta::Pairs(_) => Slides::Empty,
        }
    }

    ///force-clear without exhausting — the caller skipped the expansion
    pub fn clear(&mut self) {
        self.event = Delta::Empty;
        self.translator = Translator::new();
        self.pair_state = PairState::Idle;
        self.chase_state = PairState::Idle;
    }

    ///record a plain hole slide under the block's translator at record
    ///time (blockmap records the same events over its own slots)
    pub(crate) fn record_none_slide(&mut self, translator: Translator<A>, from: Pos, to: Pos) {
        self.translator = translator;
        self.pair_state = PairState::Idle;
        self.chase_state = PairState::Idle;
        self.event = Delta::NoneSlide { from, to };
    }

    ///record a fetch-gather: hole i compacts to `open ± spread(i)` (right
    ///+ / left −), nearest first — the STRIDED landings leave a sentinel
    ///slot every 15 holes (≥1 live per 16 across the banked range; the
    ///expansion derives the sentinel element moves from the stride gaps).
    ///the payload reuses `buf` — no per-bank alloc
    pub(crate) fn record_gather(
        &mut self,
        translator: Translator<A>,
        open: usize,
        right: bool,
        holes: &[usize],
    ) {
        self.translator = translator;
        self.buf.clear();
        self.buf
            .extend(holes.iter().enumerate().map(|(i, &h)| {
                let spread = i + i / 15;
                NoneSlide::new(Pos(h), Pos(if right { open + spread } else { open - spread }))
            }));
        self.pair_state = PairState::Idle;
        self.chase_state = PairState::Idle;
        self.event = Delta::Gather { n: holes.len() };
    }

    ///record a region rebalance. `mask` = the region's pre-move occupancy,
    ///`at` = k's rank in it (the lattice reserves u(at) for k), `stride`
    ///≥ 2 (the lattice fit is rebalance_region's guarantee)
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
        self.pair_state = PairState::Idle;
        self.chase_state = PairState::Idle;
        self.event = Delta::Rebalance { base, at, stride, mask };
    }
}

///`Deltas::iter`'s return — pulls name pairs through the block-owned
///expansion, clearing at exhaust
pub struct DeltaIter<'a, A>(&'a mut Deltas<A>);

impl<A: UnsignedNum> Iterator for DeltaIter<'_, A> {
    type Item = (A, A);

    fn next(&mut self) -> Option<(A, A)> {
        let translator = self.0.translator;
        self.0
            .next_pos()
            .map(|(f, t)| (translator.p2a(f), translator.p2a(t)))
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

//dead with the 2026-10-22 gather swap — the ladder subdivide/slides/
//banks/mints; no word lending, no streak banking
///the borrow line: a word lends holes while under 5/6 of capacity
///(53 of 64). borrows milk rebalanced words toward High before the
///next rebalance — ~5 lends per stride-4-lattice word per cycle
//const HIGH: u32 = 53;

///holes per borrow event: lendable guarantees ≥ 11 clear bits
///(popcount < HIGH), so one event banks 10 beside k — the old fetch's
///amortization bounded to the lending word
//const BORROW_MIN: usize = 10;

///the bank fork: a streak this hot banks (concentrate) instead of
///balancing or sliding (spread) — clusters amortize via banks, scattered
///inserts level via rebalances (balancing wins random, banking wins
///runs — measured both ways, twice: the original ladder 2026-10-20, and
///the 2026-10-23 single-op sweep where each arm alone lost the other
///workload: bank-only random 572 vs 293, rebalance-only runs 5419 vs 68)
const STREAK_BANK: usize = 2;

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
pub(crate) const FREE_DEN: usize = 6;

//only SkipSet's STRIDE stays a power of two (its slot math is
//shift/mask); the one divide left on FREE_STRIDE is the fetch count
const _: () = assert!(FREE_STRIDE >= 2, "FREE_STRIDE must be ≥ 2");

//z² variance gate — dead with the 2026-10-22 trigger swap (the
//reversal trigger is pos_sum < len·log2(len), no spread term)
//const REVERSE_Z2: u128 = 4;

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


///block-backed occupancy words — the set's fixed mask region. `Mask`'s
///algorithm suite over the block bytes (mask.rs keeps the v1 Vec form for
///the untouched tiers; this port cannot reshape it, so the suite is
///mirrored here and blockset's tests exercise it in place). bit p ⇔ slot
///p live, bits past `bits` clear. the physical word array is preallocated
///for `covered` slots and NEVER grows — `covered` is the block-gained
///invariant the //! header documents. words past the tracked range stay
///zero (the block's zero-record invariant keeps unwritten claims zero)
struct SlotMask<P> {
    s:       S<P>,
    bits:    usize,
    covered: usize,
}

impl<P: Ptr> SlotMask<P> {
    ///the region as u64 words. SAFETY: every claimed word is initialized —
    ///the region is written (live words) or zeroed by the block (zero-
    ///record invariant covers unwritten claims)
    fn words<'a>(&self, b: &'a Block<P>) -> &'a [u64] {
        let buf = b.stack_buf::<u64>(self.s);
        unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len()) }
    }

    fn words_mut<'a>(&self, b: &'a mut Block<P>) -> &'a mut [u64] {
        let buf = b.stack_buf_mut::<u64>(self.s);
        unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast(), buf.len()) }
    }

    ///word count of the TRACKED range (the scans' bound — the physical
    ///array is longer)
    #[inline]
    fn words_for(&self) -> usize {
        (self.bits + 63) / 64
    }

    ///retrack to `bits` ≤ covered — set bits preserved, words preexist
    fn resize(&mut self, bits: usize) {
        assert!(bits <= self.covered, "mask resize past the fixed region");
        self.bits = bits;
    }

    #[inline]
    fn get(&self, b: &Block<P>, p: usize) -> bool {
        assert!(p < self.bits, "mask index {p} ≥ {}", self.bits);
        self.words(b)[p >> 6] & 1 << (p & 63) != 0
    }

    #[inline]
    fn set(&self, b: &mut Block<P>, p: usize, live: bool) {
        assert!(p < self.bits, "mask index {p} ≥ {}", self.bits);
        let word = &mut self.words_mut(b)[p >> 6];
        if live {
            *word |= 1 << (p & 63);
        } else {
            *word &= !(1 << (p & 63));
        }
    }

    ///first set bit ≥ from, or None
    #[inline]
    fn next_set(&self, b: &Block<P>, from: usize) -> Option<usize> {
        let words = self.words(b);
        //dense fast path: the bit at `from` is usually set — `from` is the
        //answer, no scan and no position rebuild
        let mut w = from >> 6;
        let mut off = from & 63;
        if w < self.words_for() && words[w] & 1 << off != 0 {
            return Some(from);
        }
        while w < self.words_for() {
            let word = words[w] >> off;
            if word != 0 {
                return Some((w << 6) + off + word.trailing_zeros() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    ///highest set bit < from, or None
    #[inline]
    fn prev_set(&self, b: &Block<P>, from: usize) -> Option<usize> {
        let from = from.min(self.bits);
        if from == 0 {
            return None;
        }
        let words = self.words(b);
        //dense fast path: the bit just below is usually set
        let p = from - 1;
        if words[p >> 6] & 1 << (p & 63) != 0 {
            return Some(p);
        }
        let mut w = p >> 6;
        let mut keep = (p & 63) + 1;
        loop {
            let word = words[w] & if keep == 64 { u64::MAX } else { (1 << keep) - 1 };
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
            if w == 0 {
                return None;
            }
            w -= 1;
            keep = 64;
        }
    }

    ///highest set bit, or None
    fn last_set(&self, b: &Block<P>) -> Option<usize> {
        let words = self.words(b);
        let mut w = self.words_for();
        while w > 0 {
            w -= 1;
            let word = words[w];
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
        }
        None
    }

    ///the word containing p — 0 past the tracked range. the cursor
    ///steppers' cache load
    #[inline]
    fn word_at(&self, b: &Block<P>, p: usize) -> u64 {
        self.words(b).get(p >> 6).copied().unwrap_or(0)
    }

    ///first set bit strictly above p — the cursor's cache-bearing step
    ///(VALUE-passed word; see mask.rs step_up)
    #[inline]
    fn step_up(&self, b: &Block<P>, p: usize, word: u64) -> Option<(usize, u64)> {
        let hi = p & 63;
        if hi < 63 {
            if word & 1 << (hi + 1) != 0 {
                return Some((p + 1, word));
            }
            let rest = word >> (hi + 1);
            if rest != 0 {
                return Some(((p & !63) + (hi + 1) + rest.trailing_zeros() as usize, word));
            }
        }
        let q = self.next_set(b, p + 1)?;
        Some((q, self.word_at(b, q)))
    }

    ///first set bit strictly below p — the mirrored step (cache contract
    ///as step_up)
    #[inline]
    fn step_down(&self, b: &Block<P>, p: usize, word: u64) -> Option<(usize, u64)> {
        let lo = p & 63;
        if lo > 0 {
            if word & 1 << (lo - 1) != 0 {
                return Some((p - 1, word));
            }
            let below = word & ((1 << (lo - 1)) - 1);
            if below != 0 {
                return Some(((p & !63) + 63 - below.leading_zeros() as usize, word));
            }
        }
        let q = self.prev_set(b, p)?;
        Some((q, self.word_at(b, q)))
    }

    ///first clear bit ≥ from and < bits, or None
    #[inline]
    fn next_clear(&self, b: &Block<P>, from: usize) -> Option<usize> {
        if from >= self.bits {
            return None;
        }
        let words = self.words(b);
        //fast path: `from` itself clear — it's the answer
        if words[from >> 6] & 1 << (from & 63) == 0 {
            return Some(from);
        }
        let mut w = from >> 6;
        let mut off = from & 63;
        while w < self.words_for() {
            let end = (self.bits - (w << 6)).min(64);
            let mut word = !words[w] >> off;
            if end < 64 {
                //shift already dropped `off` low bits of a partial word
                word &= (1 << (end - off)) - 1;
            }
            if word != 0 {
                return Some((w << 6) + off + word.trailing_zeros() as usize);
            }
            w += 1;
            off = 0;
        }
        None
    }

    ///last clear bit ≤ from (clamped to bits−1), or None
    #[inline]
    fn prev_clear(&self, b: &Block<P>, from: usize) -> Option<usize> {
        if self.bits == 0 {
            return None;
        }
        let from = from.min(self.bits - 1);
        let words = self.words(b);
        //fast path: `from` itself clear — it's the answer
        if words[from >> 6] & 1 << (from & 63) == 0 {
            return Some(from);
        }
        let mut w = from >> 6;
        let mut keep = from & 63;
        loop {
            let end = (self.bits - (w << 6)).min(64);
            let mut word = !words[w];
            word &= if keep == 63 { u64::MAX } else { (1 << (keep + 1)) - 1 };
            if end < 64 {
                word &= (1 << end) - 1;
            }
            if word != 0 {
                return Some((w << 6) + 63 - word.leading_zeros() as usize);
            }
            if w == 0 {
                return None;
            }
            w -= 1;
            keep = 63;
        }
    }

    ///nearest clear bit to `center`, ties to the after side. `center`
    ///itself is excluded — callers pass a position they occupy
    #[inline]
    fn nearest_clear(&self, b: &Block<P>, center: usize) -> Option<usize> {
        let after = if center < self.bits {
            self.next_clear(b, center + 1)
        } else {
            None
        };
        let before = center.checked_sub(1).and_then(|p| self.prev_clear(b, p));
        match (after, before) {
            (Some(a), Some(x)) => Some(if a - center <= center - x { a } else { x }),
            (a, x) => a.or(x),
        }
    }

    ///set bits in ascending order / descending order — the exhaustion
    ///zeroing of mask.rs's engines is preserved (subtle_bugs §21)
    fn iter_set<'a>(&self, b: &'a Block<P>) -> SetBitsW<'a> {
        let mut words = self.words(b).iter();
        SetBitsW { word: words.next().copied().unwrap_or(0), words, pos: 0 }
    }

    fn iter_set_rev<'a>(&self, b: &'a Block<P>) -> SetBitsRevW<'a> {
        let mut words = self.words(b).iter().rev();
        SetBitsRevW { word: words.next().copied().unwrap_or(0), words, pos: self.bits.wrapping_sub(1) }
    }

    ///number of set bits (tracked range)
    #[allow(dead_code)]
    fn count(&self, b: &Block<P>) -> usize {
        self.words(b)[..self.words_for()].iter().map(|w| w.count_ones() as usize).sum()
    }

    ///region occupancy [from, to) → the v1 delta payload Mask (rare-path,
    ///per-bit — the record sites only)
    fn region_mask(&self, b: &Block<P>, from: usize, to: usize) -> Mask {
        assert!(from <= to && to <= self.bits, "region [{from}, {to}) out of range");
        let mut region = Mask::with_bits(to - from);
        for p in from..to {
            if self.words(b)[p >> 6] & 1 << (p & 63) != 0 {
                region.set(p - from, true);
            }
        }
        region
    }

    ///the rotation turn's mask half: bit p → 2·p ± the band ripple (see
    ///mask.rs rotate_mask — the doubled range must fit the fixed region,
    ///grow_rotate's `new_cap ≤ slot_max` guard). panics if bits > midpoint
    ///(or, reversed, offset ≥ midpoint)
    fn rotate_mask(&mut self, b: &mut Block<P>, offset: usize, midpoint: usize, reversed: bool) {
        assert!(self.bits <= midpoint, "bits {} > midpoint {midpoint}", self.bits);
        let new_words = (self.bits * 2 + 63) / 64;
        assert!(new_words <= self.words(b).len(), "doubled range past the fixed mask region");
        let old_words = self.words_for();
        let cap = self.bits;
        self.bits = cap * 2;
        {
            let words = self.words_mut(b);
            for i in (0..old_words).rev() {
                let w = words[i];
                words[i * 2] = spread(w as u32);
                //word 2i+1 only when the doubled tracked range covers it —
                //its spread names bits ≥ the old cap, zero by the
                //no-bit-past-bits invariant (word-multiple caps lose the
                //scratch word the resize-then-truncate form allocated)
                if i * 2 + 1 < new_words {
                    words[i * 2 + 1] = spread((w >> 32) as u32);
                }
            }
        }
        //band target range + shift direction (mask.rs's mapping)
        let (lo, hi, down) = if reversed {
            assert!(offset < midpoint, "reversed offset {offset} ≥ midpoint {midpoint}");
            (offset * 2 + 1, cap * 2, true)
        } else if offset <= midpoint {
            (0, offset.min(cap) * 2, false)
        } else {
            match offset - midpoint {
                s if s < cap => (s * 2, cap * 2, false),
                _ => (1, 0, false),
            }
        };
        if lo >= hi {
            return;
        }
        let wlo = lo >> 6;
        let whi = (hi - 1) >> 6;
        //down: the word above's bit-0 element falls onto this word's bit 63
        let mut fall = 0u64;
        let words = self.words_mut(b);
        for wi in (wlo..=whi).rev() {
            let low = if wi == wlo { lo & 63 } else { 0 };
            let high = if wi == whi { ((hi - 1) & 63) + 1 } else { 64 };
            let rmask = if high == 64 {
                if low == 0 { u64::MAX } else { u64::MAX << low }
            } else {
                ((1u64 << (high - low)) - 1) << low
            };
            let w = words[wi];
            let ripple = if down {
                (w & rmask) >> 1 & rmask
            } else {
                (w & rmask) << 1 & rmask
            };
            let mut nw = ripple | w & !rmask;
            if fall != 0 {
                nw |= 1 << 63;
            }
            words[wi] = nw;
            fall = if down { w & rmask & 1 } else { 0 };
        }
    }

    ///bit p ⇔ old bit bits−1−p (the array-reversal half of a flip; the
    ///trailing zero slack shifts down — mask.rs reverse)
    fn reverse(&self, b: &mut Block<P>) {
        let n = self.words_for();
        let words = self.words_mut(b);
        for i in 0..n / 2 {
            words.swap(i, n - 1 - i);
        }
        for w in words[..n].iter_mut() {
            *w = w.reverse_bits();
        }
        let slack = (64 - self.bits % 64) % 64;
        if slack != 0 {
            let mut below = 0;
            for w in words[..n].iter_mut().rev() {
                let cur = *w;
                *w = (cur >> slack) | (below << (64 - slack));
                below = cur & ((1 << slack) - 1);
            }
        }
    }
}

///`SlotMask::iter_set`'s engine (mask.rs's SetBits — exhaustion zeroes the
///cache, §21). pub only for Iter's field visibility; not API
pub struct SetBitsW<'a> {
    words: std::slice::Iter<'a, u64>,
    word:  u64,
    pos:   usize,
}

impl Iterator for SetBitsW<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        loop {
            //dense common case: `pos` is the state, the yield is free
            if self.word & 1 << (self.pos & 63) != 0 {
                let p = self.pos;
                self.pos += 1;
                if self.pos & 63 == 0 {
                    self.word = self.words.next().copied().unwrap_or(0);
                }
                return Some(p);
            }
            //hole: advance one bit (tz-jumping measured slower — mask.rs)
            if self.word >> (self.pos & 63) == 0 {
                self.pos = (self.pos | 63) + 1;
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = 0;
                        return None;
                    }
                }
            } else {
                self.pos += 1;
            }
        }
    }
}

///the descending mirror (mask.rs's SetBitsRev). pub only for Iter's field
///visibility; not API
pub struct SetBitsRevW<'a> {
    words: std::iter::Rev<std::slice::Iter<'a, u64>>,
    word:  u64,
    pos:   usize,
}

impl Iterator for SetBitsRevW<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        loop {
            let bit = 1u64 << (self.pos & 63);
            if self.word & bit != 0 {
                let p = self.pos;
                self.pos = self.pos.wrapping_sub(1);
                if self.pos & 63 == 63 {
                    self.word = self.words.next().copied().unwrap_or(0);
                }
                return Some(p);
            }
            let below = self.word & (bit | bit.wrapping_sub(1));
            if below == 0 {
                self.pos = (self.pos & !63).wrapping_sub(1);
                match self.words.next() {
                    Some(&w) => self.word = w,
                    None => {
                        self.word = 0;
                        return None;
                    }
                }
            } else {
                self.pos = self.pos.wrapping_sub(1);
            }
        }
    }
}

///slots — 2i; mask.rs's (v1-shared) spread is private to that module
fn spread(x: u32) -> u64 {
    let mut x = x as u64;
    x = (x | (x << 16)) & 0x0000_FFFF_0000_FFFF;
    x = (x | (x << 8)) & 0x00FF_00FF_00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333;
    x = (x | (x << 1)) & 0x5555_5555_5555_5555;
    x
}

///slot-walk state machine — mask.rs's MaskCursor shape over the block
///words (dir/slot/ghost/word cache; the set/map cursors wrap it)
struct SlotCursor {
    dir:  bool,
    ///the current slot, or the forward ghost
    slot: Option<usize>,
    word: u64,
}

impl SlotCursor {
    fn start(dir: bool) -> Self {
        SlotCursor { dir, slot: None, word: 0 }
    }

    fn dir(&self) -> bool {
        self.dir
    }

    ///land at a live slot — the cache load
    #[inline]
    fn at<P: Ptr>(&mut self, m: &SlotMask<P>, b: &Block<P>, q: usize) {
        self.word = m.word_at(b, q);
        self.slot = Some(q);
    }

    ///the forward ghost
    fn end(&mut self) {
        self.slot = None;
    }

    #[inline]
    fn slot(&self) -> Option<usize> {
        self.slot
    }

    ///step past slot `p` (yield-then-step, `next`'s half — the caller
    ///holds p from slot()); past the last slot ghosts
    #[inline]
    fn advance<P: Ptr>(&mut self, m: &SlotMask<P>, b: &Block<P>, p: usize) {
        let q = if self.dir {
            m.step_up(b, p, self.word)
        } else {
            m.step_down(b, p, self.word)
        };
        match q {
            Some((q, w)) => {
                self.word = w;
                self.slot = Some(q);
            }
            None => self.slot = None,
        }
    }

    ///step back (step-then-yield, `prev`'s half): the slot now current, or
    ///None at the walk's start (state stays put; from the ghost, resumes
    ///at the far end)
    #[inline]
    fn retreat<P: Ptr>(&mut self, m: &SlotMask<P>, b: &Block<P>) -> Option<usize> {
        let p = match self.slot {
            None => {
                let q = if self.dir { m.last_set(b)? } else { m.next_set(b, 0)? };
                self.at(m, b, q);
                return Some(q);
            }
            Some(p) => p,
        };
        let q = if self.dir { m.step_down(b, p, self.word) } else { m.step_up(b, p, self.word) };
        let (r, w) = q?;
        self.word = w;
        self.slot = Some(r);
        Some(r)
    }
}

///dense ordered set over archived keys: pos-ordered slot array holding
///`K::Handle` copies over the set's OWN `Block<P>` — stack carries the
///fixed occupancy words (first item, sized for the slot ceiling) then the
///growing Handle slots (last item); K's archive hangs heap bytes off the
///same block. mask bit p ⇔ slot p live.
pub struct BlockSet<K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr = u32> {
    translator: Translator<A>,
    ///the whole tier — stack [mask words | slots], heap = K's archived keys
    block:      Block<P>,
    ///fixed occupancy words (first stack item — never moves, never grows)
    mask:       SlotMask<P>,
    ///the slot region (LAST stack item — grows by extend_buf)
    data_s:     S<P>,
    ///the slot ceiling — the fixed mask's coverage and every ladder
    ///ceiling's ceiling (min of the constructor's ask and the name space)
    slot_max:   usize,
    len:        usize,
    ///running sum of placed slot positions — the mean-insert-position
    ///signal for the reversal tier
    pos_sum:    u64,
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
    ///K's defining site — the storage never names it (reads are through
    ///RelOrd over K::Handle)
    _k:         PhantomData<fn() -> K>,
}

impl<K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr> BlockSet<K, A, P> {
    ///slot ceiling = min(name space, DEFAULT_SLOTS), the pointer-width
    ///clamp applied — growth-by-turn matches the old Vec doubling inside
    ///the fixed mask's coverage
    pub fn new() -> Self {
        Self::construct(Self::default_slots())
    }

    ///explicit slot ceiling (≤ the name space) — the fixed mask's coverage
    pub fn with_capacity(slots: usize) -> Self {
        Self::construct(slots)
    }

    fn default_slots() -> usize {
        A::MAX.as_usize().saturating_add(1).min(DEFAULT_SLOTS)
    }

    fn construct(slots: usize) -> Self {
        let name_space = A::MAX.as_usize().saturating_add(1);
        let handle = size_of::<K::Handle>();
        //clamp: the fixed mask + the full data region must fit one block.
        //closed-form width floor (8·⌈s/64⌉ ≤ 8 → per-slot cost ≥ handle +
        //1/8), then trim — never an underflow, even for a mask that alone
        //exceeds the pointer width
        let mut smax = slots
            .min(name_space)
            .min(8 * pmax::<P>().saturating_sub(8) / (8 * handle + 1));
        while 8 * smax.div_ceil(64) + smax * handle > pmax::<P>() {
            smax = smax.saturating_sub(1);
        }
        let mask_bytes = 8 * smax.div_ceil(64);
        //born at the mask + the first 4 slots — data growth beyond that
        //takes the block's own growth (fit)
        let mut block = Block::new(mask_bytes + 4 * handle);
        let mask_s = block.push_buf::<u64>(smax.div_ceil(64)).expect("mask region");
        //0-len slot item — the stack's LAST, extends from here on
        let data_s = block.push_buf::<K::Handle>(0).expect("slot region");
        BlockSet {
            translator: Translator::new(),
            block,
            mask: SlotMask { s: mask_s, bits: 0, covered: smax },
            data_s,
            slot_max: smax,
            len: 0,
            pos_sum: 0,
            epoch: 0,
            last_insert: 0,
            streak: 0,
            deltas: Deltas::new(),
            _k: PhantomData,
        }
    }

    ///the handle slots as a raw view — mask-governed liveness (mask ⇔ init)
    fn data(&self) -> &[MaybeUninit<K::Handle>] {
        self.block.stack_buf::<K::Handle>(self.data_s)
    }

    fn data_mut(&mut self) -> &mut [MaybeUninit<K::Handle>] {
        self.block.stack_buf_mut::<K::Handle>(self.data_s)
    }

    ///slots allocated (mask bit range — the old `data.len()`)
    pub fn cap(&self) -> usize {
        self.data_s.1.as_usize() / size_of::<K::Handle>()
    }

    ///the block-level capacity feedback: grow to fit `extra` stack-or-heap
    ///bytes — doubling, clamped to the pointer width. Err at the width
    ///ceiling (data growth panics from there: the constructor asserts the
    ///data budget inside it; only heap pressure can starve a data grow,
    ///the arena tier's escape — no new ladder bail-outs)
    fn fit(&mut self, extra: usize) -> Result<(), ()> {
        let pmax = pmax::<P>();
        let need = self.block.stack_len() + self.block.heap_len() + extra;
        if need > pmax {
            return Err(());
        }
        self.block.grow(need.max(self.block.cap() * 2).min(pmax)).map_err(|_| ())
    }

    ///extend the slot region (LAST stack item) by `add` slots; grows the
    ///block on CapErr. the mask's tracked range belongs to the CALLER —
    ///plain extensions resize it (push_slot), the turn's rotate_mask
    ///doubles it across this very extension (a resize here would
    ///double-count)
    fn grow_data(&mut self, add: usize) {
        loop {
            if let Ok(s) = self.block.extend_buf::<K::Handle>(self.data_s, add) {
                self.data_s = s;
                return;
            }
            self.fit(add * size_of::<K::Handle>())
                .expect("block byte capacity exhausted");
        }
    }

    ///archive k into the block's heap — try-then-rewind: CapErr rolls the
    ///heap front back (the block wipes it) and Err(k) is the only silent-
    ///failure path in this tier, so fronts stay untouched on Err. heap
    ///pressure gets ONE block-grow retry (the fronts share one gap — the
    ///same doubling budget the data rung pays) before the rewind is final
    fn archive_once(&mut self, k: &K) -> Result<K::Handle, ()> {
        let mut al = self.block.allocator();
        K::archive(k, &mut al).map_err(|_| ())
    }

    fn archive_key(&mut self, k: K) -> Result<K::Handle, K> {
        let snap = self.block.allocator().snapshot();
        match self.archive_once(&k) {
            Ok(h) => Ok(h),
            Err(()) => {
                self.block.allocator().rewind(snap.clone());
                if self.fit(0).is_ok() {
                    if let Ok(h) = self.archive_once(&k) {
                        return Ok(h);
                    }
                    self.block.allocator().rewind(snap);
                }
                Err(k)
            }
        }
    }

    ///the block's event slot — pull `iter` to apply (exhausting clears);
    ///the next insert panics while an expansion is left half-consumed
    pub fn deltas(&mut self) -> &mut Deltas<A> {
        &mut self.deltas
    }

    pub fn len(&self) -> usize {
        self.len
    }

    ///the slot ceiling in NAMES — the old static bound (the legacy-gated
    ///tiers read it); the ladder's live ceiling is `slot_max` per instance
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
    ///subtle_bugs §17). the raw slot handle — no heap read
    pub fn get<'s>(&'s self, addr: A) -> Option<&'s K::Handle> {
        let p = self.translator.a2p(addr);
        (p.0 < self.cap() && self.mask.get(&self.block, p.0))
            .then(|| unsafe { self.key(p.0) })
    }

    ///compare a stored handle against a query key — the tiers' compare
    ///(RelOrd over the block's heap read view; &self throughout)
    pub fn cmp_key(&self, h: &K::Handle, k: &K) -> Ordering {
        K::rel_cmp(h, k, &Heap::new(&self.block))
    }

    ///the block's heap read seam — handle→bytes lookups (v2 handles carry
    ///no view; readers derive their bytes through this)
    pub fn heap(&self) -> Heap<'_, P> {
        Heap::new(&self.block)
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

    ///the block's translator snapshot (pub(crate): the arena's split
    ///event carries from/to snapshots)
    #[allow(dead_code)] //arena split event
    pub(crate) fn translator(&self) -> Translator<A> {
        self.translator
    }

    ///the block's key-min handle (Asc: first live, Desc: last live) —
    ///rare calls only (min-changing inserts, splits). the handle, not a
    ///materialized key: v2 orders through RelOrd
    pub fn min_key(&self) -> Option<K::Handle> {
        let p = if self.translator.reversed() {
            self.mask.last_set(&self.block)?
        } else {
            self.mask.next_set(&self.block, 0)?
        };
        // SAFETY: mask names a live slot
        Some(unsafe { *self.key(p) })
    }

    ///cleave at position `at`: the right half's slots copy AS-IS — data,
    ///mask bits, and translator (the offset shift preserves every name;
    ///only the block id changes, see arenaset's split event). no
    ///laddering, no re-archiving of positions. live-or-not: holes copy
    ///as holes. the right half's heap is a VERBATIM tail copy of this
    ///block's whole heap — handles resolve through it unchanged (H is
    ///end-relative: the same heap length at the tail reproduces every
    ///offset, nested regions included). the v1 compact rebuild + handle
    ///recompute is deferred: it needs a handle-rebase contract on K
    ///(v2 has no materialize). the left half keeps its own heap — its
    ///handles never moved; the right half's leaked dead bytes ride the
    ///removal-leak rule. BOTH halves reset their signals to the uniform
    ///presumption over their new lens (the moved half's insert history
    ///predicts nothing about either half's future). the shifted offset
    ///can exceed the half's cap (offset_src < at wraps — most Asc blocks
    ///carry offset 0, so the half is born at offset = cap): sound while
    ///Asc — forward turns are monotone for any offset — but REVERSED
    ///turns die there (§30), so a Desc half flips to Asc at the end and
    ///grow_rotate REFUSES the reversed turn at offset ≥ midpoint,
    ///routing the space need to the ceiling valve instead
    #[allow(dead_code)] //arena-facing (tests drive it)
    pub(crate) fn cleave(&mut self, at: usize) -> BlockSet<K, A, P> {
        debug_assert!(at < self.cap(), "cleave boundary inside the block");
        let half = self.cap() - at;
        let heap_len = self.block.heap_len();
        //the half's ceiling: the source's remaining coverage (the fixed
        //mask must cover future turns), never above the name space
        let smax = (self.slot_max - at).min(A::MAX.as_usize().saturating_add(1)).max(half);
        let mut right = BlockSet::<K, A, P>::construct(smax);
        //the shift is the split event: position ≥ split_pos ⇒ new block
        //id, SAME name (§31)
        right.translator = self.translator.shifted(at);
        right.last_insert = self.last_insert.saturating_sub(at);
        //slot region first (the fit accounts heap after it), then the
        //verbatim heap copy fits its own budget
        right.grow_data(half);
        right.mask.resize(half);
        if heap_len > 0 {
            right.fit(heap_len).expect("heap copy fits the half");
            let whole = self.block.heap(H(P::from_usize(heap_len), P::from_usize(heap_len)));
            let mut al = right.block.allocator();
            al.copy_from::<u8>(whole).expect("heap copy fits the half");
        }
        //slots copy raw — the mask half governs validity; stale hole
        //bytes stay masked-out, never read
        for p in at..self.cap() {
            let live = self.mask.get(&self.block, p);
            if live {
                right.data_mut()[p - at] = self.data()[p];
                right.mask.set(&mut right.block, p - at, true);
            }
        }
        right.len = right.mask.count(&right.block);
        for p in at..self.cap() {
            self.mask.set(&mut self.block, p, false);
        }
        self.len -= right.len;
        right.seed_uniform_signals();
        self.seed_uniform_signals();
        //the flipped half keeps offset ≥ cap (the wrap RELOCATES to the
        //mirrored value, it doesn't clear) — sound while Asc; the
        //reversed-turn refusal in grow_rotate is the live guard (§30)
        if right.translator.reversed() {
            right.reverse();
        }
        right
    }

    ///the uniform presumption: a block whose insert history is unknown
    ///seeds its z-gate with the mean/spread of uniform arrivals over
    ///[0, len) — a pos_sum of 0 at a born-big len reads maximally
    ///front-biased and flips on the first insert
    #[allow(dead_code)]
    fn seed_uniform_signals(&mut self) {
        let n = self.len as u128;
        self.pos_sum = (n * (n - 1) / 2) as u64;
        //self.pos_sq_sum = n * (n - 1) * (2 * n - 1) / 6;
        self.streak = 0;
    }

    ///k beyond the tail extreme — the end a sequential insert extends.
    ///SkipSet::insert routes these to push_beyond, skipping its descent
    pub fn beyond_tail(&self, k: &K) -> bool {
        if self.len == 0 {
            return true;
        }
        let tail = self.key_tail();
        let ord = self.cmp_key(tail, k);
        if self.translator.reversed() {
            ord == Ordering::Greater
        } else {
            ord == Ordering::Less
        }
    }

    ///the reversal trigger — mean insert position below log2(len) past
    ///the warmup (see reverse_due). SkipSet drives reversal manually
    ///and reads this; its insert path never auto-flips (see insert_at).
    ///blockmap keeps the old pos_sum·8 < len² line — an intentional
    ///divergence (its ladder never carried the z-gate); the port is
    ///deferred
    pub fn should_reverse(&self) -> bool {
        self.reverse_due()
    }

    ///mean insert position below log2(len): an O(len) flip costs one
    ///growth's worth of element copies (each element pays log2(len)
    ///copies to doubling, amortized), so it pays exactly when the
    ///stream is front-biased past that line
    fn reverse_due(&self) -> bool {
        if self.len <= 4 * FREE_STRIDE {
            return false;
        }
        let len = self.len as u64;
        self.pos_sum < len * len.ilog2() as u64
    }

    ///live handle at a base position — None if out of range or a hole.
    ///the tiers' incremental re-read
    pub fn slot(&self, pos: usize) -> Option<K::Handle> {
        (pos < self.cap() && self.mask.get(&self.block, pos))
            .then(|| unsafe { *self.key(pos) })
    }

    ///first live position ≥ from — None past the tail. the tiers'
    ///window-first sampling (tier0 slot j = the first item in window j,
    ///so banking/turns/bands can never empty a sample)
    pub fn first_live_pos(&self, from: usize) -> Option<usize> {
        self.mask.next_set(&self.block, from)
    }

    ///first live position < below — None at the head. skipset's update
    ///hint walk (the prev-mirror of first_live_pos)
    pub fn prev_live_pos(&self, below: usize) -> Option<usize> {
        self.mask.prev_set(&self.block, below)
    }

    ///live-run lengths between holes (position order) — the desert
    ///census: banking clusters holes and the solid runs it leaves
    ///behind are what the fetch walks. NOT API
    #[doc(hidden)]
    pub fn live_run_lengths(&self) -> Vec<usize> {
        let mut runs = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut prev = 0usize;
        for pos in self.mask.iter_set(&self.block) {
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
            self.mask.prev_set(&self.block, to)?
        } else {
            self.mask.next_set(&self.block, from).filter(|probe| *probe < to)?
        };
        loop {
            // SAFETY: every step lands on a mask-named slot
            match self.cmp_key(unsafe { self.key(p) }, k) {
                Ordering::Less => {}
                Ordering::Equal => return Some(self.translator.p2a(Pos(p))),
                Ordering::Greater => return None,
            }
            let q = if rev {
                let q = self.mask.prev_set(&self.block, p)?;
                if q < from {
                    return None;
                }
                q
            } else {
                let q = self.mask.next_set(&self.block, p + 1)?;
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
            let ord = self.cmp_key(tail, k);
            let beyond = if self.translator.reversed() { ord == Ordering::Greater } else { ord == Ordering::Less };
            if beyond {
                return None;
            }
            //len > 0 ⇒ a first live slot exists
            let head = unsafe { self.key(self.mask.next_set(&self.block, 0).unwrap()) };
            let ord = self.cmp_key(head, k);
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

    ///first→last over live keys (raw handles — order via RelOrd)
    pub fn iter(&self) -> Iter<'_, K, A, P> {
        //direction picked once, never per element; Desc blocks: key order
        //ascends against pos order, so they walk positions down
        if self.translator.reversed() {
            Iter::Desc { bits: self.mask.iter_set_rev(&self.block), set: self }
        } else {
            Iter::Asc { bits: self.mask.iter_set(&self.block), set: self }
        }
    }

    ///cursor at the first key ≥ k, or the walk's end if none
    pub fn seek(&self, k: &K) -> Cursor<'_, K, A, P> {
        let mut cursor = Cursor::new(self);
        cursor.seek(k);
        cursor
    }

    ///cursor at the first key > k — k itself lands in a downward walk
    pub fn seek_after(&self, k: &K) -> Cursor<'_, K, A, P> {
        let mut cursor = Cursor::new(self);
        cursor.seek_after(k);
        cursor
    }

    ///first live slot at/after the pos boundary `from`, in KEY order —
    ///Desc runs key order against pos order, so its mirror is prev_set
    fn key_bound(&self, from: usize) -> Option<usize> {
        if self.translator.reversed() {
            self.mask.prev_set(&self.block, from)
        } else {
            self.mask.next_set(&self.block, from)
        }
    }

    ///Err returns k (dup, or out of names). heap-capacity failures ride
    ///the same Err — try-then-rewind leaves both fronts untouched
    pub fn insert(&mut self, k: K) -> Result<A, K> {
        assert!(!self.deltas.is_pending(), "pending deltas: apply + exhaust before reuse");
        if self.cap() == 0 {
            return self.insert_first(k);
        }
        if self.len >= self.slot_max {
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
        let h = self.archive_key(k)?;
        Ok(self.place_from(h, spot).0)
    }

    ///the ladder from a known spot: extend at the tail edge, open
    ///elsewhere. returns the addr with the placed position — callers
    ///holding positions skip the a2p round-trip (era-valid until the
    ///next silent move)
    fn place_from(&mut self, h: K::Handle, spot: Pos) -> (A, Pos) {
        //sequential at the back: extend instead of sliding
        if spot.0 == self.cap() && self.cap() < self.slot_max {
            (self.push_back(h, spot), spot)
        } else {
            let p = self.open(spot);
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
    #[allow(dead_code)] //skipset's consumer is legacy-gated
    pub(crate) unsafe fn insert_at(
        &mut self,
        k: K,
        from: usize,
        to: usize,
    ) -> Result<(A, Pos), K> {
        assert!(!self.deltas.is_pending(), "pending deltas: apply + exhaust before reuse");
        if self.cap() == 0 {
            let a = self.insert_first(k)?;
            return Ok((a, Pos(0)));
        }
        if self.len >= self.slot_max {
            return Err(k);
        }
        match self.bsearch_bounds(&k, from, to) {
            Ok(_) => return Err(k), //dup
            Err(spot) => {
                let h = self.archive_key(k)?;
                Ok(self.place_from(h, spot))
            }
        }
    }


    ///tail push with a caller guarantee: k is beyond the tail extreme
    ///(max Asc, min Desc — the end a sequential insert extends). no
    ///scan, no dup possible. SkipSet's beyond_tail pre-check routes
    ///appends here — one tail scan total instead of three
    #[allow(dead_code)]
    pub(crate) fn push_beyond(&mut self, k: K) -> Result<(A, Pos), K> {
        assert!(!self.deltas.is_pending(), "pending deltas: apply + exhaust before reuse");
        if self.cap() == 0 {
            let a = self.insert_first(k)?;
            return Ok((a, Pos(0)));
        }
        if self.len >= self.slot_max {
            return Err(k);
        }
        let spot = Pos(self.mask.last_set(&self.block).unwrap() + 1);
        let h = self.archive_key(k)?;
        Ok(self.place_from(h, spot))
    }

    ///in-place reversal: pos 0 = max, names follow the slots (the
    ///translator absorbs it — no deltas; a pending event stays valid).
    ///SkipSet calls this manually — plain insert auto-flips via
    ///maybe_reverse, insert_at never does
    pub(crate) fn reverse(&mut self) {
        self.data_mut().reverse();
        self.mask.reverse(&mut self.block);
        self.translator = self.translator.reverse(self.cap());
        let len = self.len as u64;
        //reflect the bias signal around len — the trigger's own scale —
        // NOT the physical mirror (cap−1−p): cap imports density noise.
        // fires only when pos_sum < len·log2(len) < len², never wrapping
        self.pos_sum = len.wrapping_mul(len).wrapping_sub(self.pos_sum);
        //positions mirrored — a silent move for pos holders. the streak
        //anchor follows its element EXACTLY (the array mirrors p →
        //cap−1−p — anything else misreads the next streak compare);
        //the trigger sums keep the len-reflection above
        self.last_insert = self
            .cap()
            .saturating_sub(1)
            .saturating_sub(self.last_insert);
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
            let p = self.mask.last_set(&self.block).unwrap();
            // SAFETY: last_set names a live slot — mask ⇔ init
            let ord = self.cmp_key(unsafe { self.key(p) }, k);
            let beyond = if self.translator.reversed() { ord == Ordering::Greater } else { ord == Ordering::Less };
            if beyond {
                return Some(Pos(p + 1));
            }
        }
        //hot streak: the previous insert landed close to its predecessor,
        //so k's bracket likely sits within a walk of it — bounded walk
        //from the anchor beats the full bsearch, falls back on any miss
        if self.streak > 0 {
            match self.spot_near(k) {
                Some(Ok(p)) => return Some(p),
                Some(Err(())) => return None,
                None => {}
            }
        }
        self.bsearch(k).err()
    }

    ///hinted bracket from `last_insert`: walk live slots within
    ///2·FREE_STRIDE of the anchor until the compare crosses k; the
    ///crossing pair's pos-lower slot + 1 is the same boundary bsearch
    ///returns. Some(Err(())) = dup; None = out of reach, caller falls
    ///back
    fn spot_near(&self, k: &K) -> Option<Result<Pos, ()>> {
        let anchor = self.last_insert;
        if !self.mask.get(&self.block, anchor) {
            return None; //removed since the streak landed
        }
        // SAFETY: mask names the anchor live
        let first = self.cmp_key(unsafe { self.key(anchor) }, k);
        if first == Ordering::Equal {
            return Some(Err(()));
        }
        let rev = self.translator.reversed();
        //Asc: k beyond the anchor's key walks pos-up, else pos-down;
        //Desc runs key order against pos order — the walk mirrors
        let up = if rev { first == Ordering::Greater } else { first == Ordering::Less };
        let bound = 2 * FREE_STRIDE;
        let mut prev = anchor;
        let mut cursor = anchor;
        loop {
            let q = if up {
                self.mask.next_set(&self.block, cursor + 1)
            } else {
                self.mask.prev_set(&self.block, cursor)
            };
            let Some(q) = q else { return None };
            if q + bound < anchor || q > anchor + bound {
                return None; //past the streak's reach
            }
            // SAFETY: the steppers name live slots
            let ord = self.cmp_key(unsafe { self.key(q) }, k);
            match ord {
                Ordering::Equal => return Some(Err(())),
                //keys are monotone along the walk — the first opposite-side
                //key IS the crossing; the bracket pair is (prev, q)
                ord if ord == first.reverse() => return Some(Ok(Pos(prev.min(q) + 1))),
                _ => {}
            }
            prev = q;
            cursor = q;
        }
    }

    ///vec-like first allocation: 4 slots, no translator turn until it
    ///fills. archive failure rolls the stack front back too — both
    ///fronts untouched on Err (the region wipes with the shrink)
    fn insert_first(&mut self, k: K) -> Result<A, K> {
        self.grow_data(4);
        self.mask.resize(self.cap());
        match self.archive_key(k) {
            Ok(h) => Ok(self.place(h, Pos(0))),
            Err(k) => {
                self.block.stack_shrink(self.data_s.0.as_usize());
                self.data_s = S(self.data_s.0, P::from_usize(0));
                self.mask.resize(0);
                Err(k)
            }
        }
    }

    ///extend the allocation and place k at the edge — dense, holes never
    ///minted here. caller guarantees spot == cap < the slot ceiling
    fn push_back(&mut self, h: K::Handle, spot: Pos) -> A {
        self.push_slot();
        self.place(h, spot)
    }

    ///open a slot at/near `spot`: turns and gathers, escalating until a
    ///hole opens
    fn open(&mut self, spot: Pos) -> Pos {
        let mut spot = spot;
        //APPEND FASTPATH: a solid tail run within a stride of the edge —
        //extend the block and shift the run up. ≤ FREE_STRIDE elements
        //move, no ladder rung, no banking; holed tails fall through (a
        //near hole within the stride slides even cheaper and grows
        //nothing), as does the spot-hole case (direct place)
        if spot.0 < self.cap()
            && self.cap() - spot.0 <= FREE_STRIDE
            && self.cap() < self.slot_max
            && self.mask.get(&self.block, spot.0)
            && (spot.0..self.cap()).all(|p| self.mask.get(&self.block, p))
        {
            let from = self.cap();
            self.push_slot();
            return self.slide_open(from, spot.0);
        }
        //preemptive turn past the ceiling (TURN_NUM/DEN): one O(n)
        //turn beats the long scans (mid-block inserts only). refused ⇒
        //the gather banks from however far — a hole always exists
        //below the ceiling (len < ceiling, mask not solid)
        if self.len.wrapping_mul(TURN_DEN) > self.cap().wrapping_mul(TURN_NUM)
            && self.cap() - spot.0 > FREE_STRIDE
        {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            }
        }
        self.gather_slots(spot)
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
        self.data_mut()[p.0] = MaybeUninit::new(h);
        self.mask.set(&mut self.block, p.0, true);
        self.len += 1;
        self.pos_sum = self.pos_sum.wrapping_add(p.0 as u64);
        //locality streak: this insert within a stride of the previous
        self.streak =
            if p.0.abs_diff(self.last_insert) <= FREE_STRIDE { self.streak + 1 } else { 0 };
        self.last_insert = p.0;
        self.translator.p2a(p)
    }

    ///None if addr names no live element. the handle is dead after this —
    ///its heap bytes leak (v0: release wiring is deferred with the
    ///freelist; the deltas/holder protocols don't model freed regions)
    pub fn remove(&mut self, addr: A) -> Option<K::Handle> {
        let pos = self.translator.a2p(addr);
        if pos.0 < self.cap() && self.mask.get(&self.block, pos.0) {
            self.mask.set(&mut self.block, pos.0, false);
            self.len -= 1;
            //the signal tracks CURRENT elements — removal subtracts its
            //slot (post-move position, not what the insert added; a
            //heuristic, wrapping keeps it total)
            self.pos_sum = self.pos_sum.wrapping_sub(pos.0 as u64);
            Some(unsafe { *self.key(pos.0) })
        } else {
            None
        }
    }

    ///in-place key swap at a live slot: archives `k`, overwrites the
    ///handle, returns the previous handle (its heap bytes leak, v0). no
    ///mask/name/signal/position changes — ORDER is the caller's
    ///contract: `k`'s key must sit inside the slot's immediate bracket.
    ///Err(k) = the archive didn't fit (try-then-rewind; state untouched)
    #[allow(dead_code)]
    pub(crate) fn replace_key(&mut self, p: usize, k: K) -> Result<K::Handle, K> {
        let h = self.archive_key(k)?;
        // SAFETY: p names a live slot (caller-checked)
        let old = unsafe { *self.key(p) };
        self.data_mut()[p] = MaybeUninit::new(h);
        Ok(old)
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
        self.bsearch_bounds(k, 0, self.cap())
    }

    ///bsearch over a position window [lo0, hi0) — insert_at's bounded
    ///search: the tier descent already narrowed the range, so this pays
    ///log2(window) probes instead of a linear walk (or the full-block
    ///log2(cap))
    fn bsearch_bounds(&self, k: &K, lo0: usize, hi0: usize) -> Result<Pos, Pos> {
        let big = self.cap() * size_of::<K::Handle>() > L2_BYTES;
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
                    let ord = self.cmp_key(probe, k);
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
                    let ord = self.cmp_key(probe, k);
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
            if self.cmp_key(unsafe { self.key(p) }, k) == Ordering::Equal {
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
            Err(spot) => Err(self.next_live(spot.0, self.cap()).map(Pos).unwrap_or(spot)),
        }
    }

    #[inline]
    fn next_live(&self, from: usize, bound: usize) -> Option<usize> {
        self.mask.next_set(&self.block, from).filter(|p| *p < bound)
    }

    ///raw live-slot read
    ///
    /// # Safety
    ///
    /// p must name a live slot — mask ⇔ init
    unsafe fn key(&self, p: usize) -> &K::Handle {
        unsafe { self.data()[p].assume_init_ref() }
    }

    ///key at the physical tail (max Asc, min Desc); panics if len == 0
    fn key_tail(&self) -> &K::Handle {
        unsafe { self.key(self.mask.last_set(&self.block).expect("live")) }
    }

    ///open a slot at/near `spot` — SUBDIVIDE first (the clear run
    ///between k's live neighbors is order-legal; interior gaps split at
    ///the middle, a tail gap fills at its head — the append edge stays
    ///solid). else the nearest hole either side decides: one plain slide
    ///within FREE_STRIDE, beyond it the WATER-LEVELING rebalance (see
    ///`rebalance_region`). a solid mask mints tail slots (len < the
    ///name ceiling guarantees room). infallible: a hole always exists,
    ///fetched or minted
    fn gather_slots(&mut self, spot: Pos) -> Pos {
        //SUBDIVIDE: the clear run between k's live neighbors — zero
        //movement. interior gaps split at the middle (either side serves
        //the next insert); a tail gap fills at its head — the append
        //edge stays solid (push_back's premise)
        let next = self.mask.next_set(&self.block, spot.0);
        let prev = self.mask.prev_set(&self.block, spot.0);
        let gap_lo = prev.map_or(0, |p| p + 1);
        let gap_hi = next.map_or(self.cap(), |n| n);
        if gap_hi > gap_lo {
            let land = if next.is_some() { gap_lo + (gap_hi - gap_lo) / 2 } else { gap_lo };
            self.deltas.record_none_slide(self.translator, Pos(land), Pos(land));
            return Pos(land);
        }
        let up = self.mask.next_clear(&self.block, spot.0 + 1);
        let down = spot.0.checked_sub(1).and_then(|b| self.mask.prev_clear(&self.block, b));
        let hole = match (up, down) {
            (Some(u), Some(d)) if u - spot.0 <= spot.0 - d => u,
            (_, Some(d)) => d,
            (Some(u), _) => u,
            //mask solid: mint tail slots — len < ceiling guarantees room
            _ => {
                let room = self.slot_max - self.cap();
                let want = (1 + (self.cap() - spot.0) / FREE_STRIDE).min(room);
                self.push_slots(want);
                self.cap() - 1
            }
        };
        if hole.abs_diff(spot.0) <= FREE_STRIDE {
            return self.slide_open(hole, spot.0);
        }
        //THE FORK: hot streaks bank — concentrated holes feed the
        //cluster's next arrivals; cold arrivals water-level — the
        //uniform spread serves random landings (measured both ways:
        //every single-op config wins one workload by losing the other).
        //NEAR holes (≤ 64) bank unconditionally — one memmove beats
        //the rebalance's machinery; locality comes from
        //want = dist/FREE_STRIDE (≤ 10 holes) plus the run-preferring
        //fetch, not from a bound
        if self.streak >= STREAK_BANK || hole.abs_diff(spot.0) <= 64 {
            //bank: want grows with the fetch distance — the next bank
            //looks proportionately less far
            let right = hole > spot.0;
            let open = if right { spot.0 } else { spot.0 - 1 };
            let bound = if right { usize::MAX } else { 0 };
            let want = hole.abs_diff(spot.0) / FREE_STRIDE;
            let mut holes = self.fetch_holes(spot, right, want, bound);
            //an empty fetch is a bug, not a fallback (§25): want ≥ 1 and
            //the walk takes the first hole unconditionally — holes[0]
            //below panics loudly if this ever regresses
            debug_assert!(!holes.is_empty());
            let seg0 = if right { holes[0] - spot.0 } else { spot.0 - 1 - holes[0] };
            holes.truncate(15 * seg0 + 1);
            let land = self.bank_landing(open, right, holes.len());
            self.compact_gather(spot, open, right, &holes);
            self.deltas.record_gather(self.translator, open, right, &holes);
            Pos(land)
        } else {
            //cold: water-level the neighborhood — the scan absorbs
            //space outward until the region's mean drops to the block's
            //(or the budget runs out) and the region is workable, the
            //lattice spreads it back uniformly, k rides at its rank.
            //the budget ≈ the dense run's size (2× the nearest hole,
            //+64 for the scan's word granularity — a hole within 31
            //must be reachable or the spot's solid word stops slack-0
            //and the ladder pays a long slide for a near hole)
            self.rebalance_region(spot, 2 * hole.abs_diff(spot.0) + 64)
        }
    }

    ///k's landing slot inside a bank's compacted hole range (right: the
    ///range is [open, open+w); left: [open+1−w, open]) — any slot in it
    ///is order-legal. CENTERED, clamped by the streak's remaining stride
    ///slack measured from `open` — where the offset actually starts: a
    ///left fetch's open is spot−1, and budgeting from the spot let Δ7
    ///landings through a Δ6 budget (measured). centering won both band
    ///directions over spot-adjacent and direction-biased landings
    ///(session_history 2026-10-21 (b)).
    fn bank_landing(&self, open: usize, right: bool, w: usize) -> usize {
        //offset from the spot-adjacent end toward the far end
        let target = (w - 1) / 2;
        let slack = FREE_STRIDE.saturating_sub(open.abs_diff(self.last_insert));
        let offset = target.min(slack);
        if right { open + offset } else { open - offset }
    }

    ///slide the hole to k's edge and record — near, borrow and forced
    ///share it. caller guarantees `hole` is the nearest on its side
    fn slide_open(&mut self, hole: usize, spot: usize) -> Pos {
        let open = if hole > spot { spot } else { spot - 1 };
        self.slide_hole(hole, open);
        self.deltas.record_none_slide(self.translator, Pos(hole), Pos(open));
        Pos(open)
    }

    ///one plain slide: hole `from` → `to`
    fn slide_hole(&mut self, from: usize, to: usize) {
        {
            let data = self.data_mut();
            // SAFETY: within the slot region; the run between is live
            unsafe {
                let ptr = data.as_mut_ptr();
                if from > to {
                    std::ptr::copy(ptr.add(to), ptr.add(to + 1), from - to);
                } else {
                    std::ptr::copy(ptr.add(from + 1), ptr.add(from), to - from);
                }
            }
        }
        //the run between is fully live (nearest hole) — two-bit mask
        //update; the stale duplicate bytes ptr::copy leaves behind are
        //masked clear, never assume_init'd
        self.mask.set(&mut self.block, from, true);
        self.mask.set(&mut self.block, to, false);
        //the previous k may ride the run — the anchor follows its element
        //(a stale anchor misreads the next streak compare)
        if from > to {
            if (to..from).contains(&self.last_insert) {
                self.last_insert += 1;
            }
        } else if (from + 1..=to).contains(&self.last_insert) {
            self.last_insert -= 1;
        }
    }

    ///one element relocation inside a rebalance pass. the pass orders
    ///make every target vacant when written (see rebalance_region); the
    ///stale source bytes are masked clear, never assume_init'd
    fn move_slot(&mut self, from: usize, to: usize) {
        let data = self.data_mut();
        data[to] = std::mem::replace(&mut data[from], MaybeUninit::uninit());
        self.mask.set(&mut self.block, from, false);
        self.mask.set(&mut self.block, to, true);
        if from == self.last_insert {
            self.last_insert = to;
        }
    }

    ///the `want` nearest holes on one side of `spot`, nearest-first
    ///(right: ascending, left: descending), within `bound` — the allowed
    ///region's far edge (right: exclusive ceiling, left: inclusive
    ///floor). RUN-PREFERRING: at the ~1/2 cycle-average density
    ///singletons ARE the floor of at-average regions, runs are the
    ///surplus of sparse ones — so once a run member is taken the walk
    ///STOPS at the next singleton (drain the run, seek no singletons);
    ///leading singletons before any run ride along (the fetched list
    ///must stay consecutive — compact_gather's solid-segment math), and
    ///`want` caps the drag. pre-read, no further scans.
    fn fetch_holes(&self, spot: Pos, right: bool, want: usize, bound: usize) -> Vec<usize> {
        //adjacent hole ⇔ the neighbor slot is clear
        let run_member = |h: usize| {
            (h > 0 && !self.mask.get(&self.block, h - 1))
                || (h + 1 < self.cap() && !self.mask.get(&self.block, h + 1))
        };
        let mut holes = Vec::with_capacity(want);
        let mut seen_run = false;
        if right {
            let mut cursor = spot.0;
            while holes.len() < want
                && let Some(h) = self.mask.next_clear(&self.block, cursor)
            {
                if h >= bound || seen_run && !run_member(h) {
                    break;
                }
                seen_run |= run_member(h);
                holes.push(h);
                cursor = h + 1;
            }
        } else {
            let mut cursor = spot.0 - 1;
            while holes.len() < want {
                let Some(h) = self.mask.prev_clear(&self.block, cursor) else { break };
                if h < bound || seen_run && !run_member(h) {
                    break;
                }
                seen_run |= run_member(h);
                holes.push(h);
                if h == 0 {
                    break;
                }
                cursor = h - 1;
            }
        }
        holes
    }

    ///gather compaction, one pass: the holes compact beside `spot` in
    ///STRIDED formation — hole i lands at `open ± (i + i/15)`, every
    ///15th slot of the banked range kept for a SENTINEL: the run's head
    ///`drag = (w−1)/15` elements land inside the range (≥1 live per 16
    ///— the skip list's stride window; the fetch truncates so the run
    ///always has drag elements to give). every member Some crosses
    ///exactly once. k lands in `open`'s first chunk.
    fn compact_gather(&mut self, spot: Pos, open: usize, right: bool, holes: &[usize]) {
        let w = holes.len();
        let drag = (w - 1) / 15;
        {
            let data = self.data_mut();
            // SAFETY: segment copies stay inside the live slot range; the
            //vacated stale sources are masked clear below, never read
            unsafe {
                let ptr = data.as_mut_ptr();
                //segment j (between consecutive holes) shifts by its crossing
                //count; far segments copy first so no live source is read
                //after being overwritten
                if right {
                    //holes ascend; segment 0 spans [spot+drag, h_0) — its
                    //head `drag` elements are the sentinels, landing later
                    for j in (0..w).rev() {
                        let (lo, len) = if j == 0 {
                            (spot.0 + drag, holes[0] - spot.0 - drag)
                        } else {
                            (holes[j - 1] + 1, holes[j] - holes[j - 1] - 1)
                        };
                        let shift = w - j;
                        std::ptr::copy(ptr.add(lo), ptr.add(lo + shift), len);
                    }
                } else {
                    //holes descend; the top segment ends at spot − 1 − drag
                    //(the element at spot never moves — k lands below it;
                    //its head `drag` elements are the sentinels)
                    for k in (0..w).rev() {
                        let (lo, len) = if k == 0 {
                            (holes[0] + 1, spot.0 - 1 - drag - holes[0])
                        } else {
                            (holes[k] + 1, holes[k - 1] - holes[k] - 1)
                        };
                        let shift = w - k;
                        std::ptr::copy(ptr.add(lo), ptr.add(lo - shift), len);
                    }
                }
            }
        }
        //sentinels land AFTER the copies (their targets sit inside the
        //copies' source span), DESCENDING — every target above the
        //remaining sentinel sources, and below the shifted block's
        //targets, so both data and pair-emission order stay safe
        {
            let data = self.data_mut();
            for i in (0..drag).rev() {
                let (src, dst) = if right {
                    (open + i, open + 16 * i + 15)
                } else {
                    (open - i, open - 16 * i - 15)
                };
                data[dst] = data[src];
            }
        }
        for &h in holes {
            self.mask.set(&mut self.block, h, true);
        }
        //the vacated target range clears; sets first — a hole compacted
        //onto its own old position ends clear. the sentinel slots re-set
        //after the clear
        if right {
            for t in open..open + w + drag {
                self.mask.set(&mut self.block, t, false);
            }
            for i in 0..drag {
                self.mask.set(&mut self.block, open + 16 * i + 15, true);
            }
        } else {
            for t in open + 1 - (w + drag)..=open {
                self.mask.set(&mut self.block, t, false);
            }
            for i in 0..drag {
                self.mask.set(&mut self.block, open - 16 * i - 15, true);
            }
        }
        //the previous k rides its segment by its crossing count — the
        //anchor follows (right: shift = w − holes below it; left:
        //shift = w − holes above it; the far side doesn't move). a
        //sentinel rides its own strided landing instead
        let anchor = &mut self.last_insert;
        if right {
            if (spot.0..spot.0 + drag).contains(anchor) {
                let i = *anchor - spot.0;
                *anchor = spot.0 + 16 * i + 15;
            } else if *anchor >= spot.0 {
                let below = holes.iter().filter(|h| **h < *anchor).count();
                *anchor += w - below;
            }
        } else if (open + 1 - drag..=open).contains(anchor) {
            let i = open - *anchor;
            *anchor = open - 16 * i - 15;
        } else if *anchor < spot.0 {
            let above = holes.iter().filter(|h| **h > *anchor).count();
            *anchor -= w - above;
        }
    }

/* dead with the 2026-10-22 gather swap — the ladder banks/mints; kept
for the blockmap port + quick revert
    ///the ceiling valve: the preemptive turn's replacement when the
    ///block cannot double (past the name midpoint, or the rotation
    ///budget spent) — nothing can be minted by growth, and repeatedly
    ///shifting O(n) elements for O(streak) holes is the worst trade in
    ///the block. instead, water-level the WHOLE block onto the
    ///minimal-stride lattice that fits len live elements (k included)
    ///below the name ceiling, extending the tail just enough — e.g.
    ///len 180, ceiling 256: stride 4 (3/4 density) spans 241, and the
    ///headroom above stays unallocated. one Rebalance event covers
    ///the block; k rides the lattice at its rank. after it, the turn
    ///line no longer holds ((s−1)/s < 5/6), so it amortizes
    fn rebalance_block(&mut self, spot: Pos) -> Pos {
        let live = self.len;
        let ceiling = Self::max_capacity();
        //minimal stride whose lattice fits: live + live/(stride−1) < ceiling
        let mut stride = 2;
        while live + live / (stride - 1) >= ceiling {
            stride += 1;
        }
        let s1 = stride - 1;
        //the tight lattice extent; the region is the WHOLE allocation
        //(end ≥ cap — lives in a trailing desert must be inside it),
        //extended only when the tight fit demands. degenerate at full
        //density: the search rounds live/(s−1) to 0 and the lattice is
        //a whole-block shift-by-one past k's rank
        let end = self.cap().max(live + live / s1 + 1);
        if end > self.cap() {
            self.push_slots(end - self.cap());
        }
        let region = self.mask.copy_region(0, end);
        //k's rank among the whole block — the boundary is count-below
        //in the pre-move mask (order is preserved)
        let r = region.count_below(spot.0.min(end).min(region.bits()));
        //the two passes exactly as the pair emission (see DeltaPairs'
        //Rebalance): right-movers descending, then left-movers ascending.
        //no FREE_STRIDE bound here — near the ceiling the minimal
        //lattice is legitimately denser than 5/6
        let target = |i: usize| i + usize::from(i >= r) + (i + usize::from(i >= r)) / s1;
        debug_assert!(target(live - 1) < end, "the lattice must fit the block");
        let mut i = live;
        for p in region.iter_set_rev() {
            i -= 1;
            let q = target(i);
            if q > p {
                self.move_slot(p, q);
            }
        }
        let mut i = 0;
        for p in region.iter_set() {
            let q = target(i);
            if q < p {
                self.move_slot(p, q);
            }
            i += 1;
        }
        self.deltas.record_rebalance(self.translator, Pos(0), r, stride, region);
        Pos(r + r / s1)
    }
*/

    ///water-leveling gather — the bank's replacement for far holes: scan
    ///words outward from `spot` (sparser side first) until the scanned
    ///region's mean density drops to the BLOCK's mean (len/cap — a
    ///denser neighborhood implies a sparser one elsewhere, the average
    ///says so) or the budget (2× the nearest hole's distance, ≈ the
    ///dense run's size) runs out; then redistribute the region's live
    ///elements PLUS k (at its rank `r`) onto the minimal-stride lattice
    ///fitting live+1 — the scanned region's own mean, whatever it came
    ///to. order-preserving; k's lattice slot is a hole by construction,
    ///so the rebalance IS the open — ONE Rebalance event covers
    ///everything. a slack-0 stop (solid scan, budget short of any hole)
    ///falls to the nearest-hole slide, however far
    fn rebalance_region(&mut self, spot: Pos, budget: usize) -> Pos {
        let mut cap = self.cap();
        //spot's word, clamped: spot == cap scans the last word, not past it
        let mut wlo = spot.0.min(cap - 1) >> 6;
        let mut whi = wlo;
        let mut live = self.mask.word_at(&self.block, wlo << 6).count_ones() as usize;
        let mut span = ((whi + 1) << 6).min(cap) - (wlo << 6);
        //the scan arms, split: the MEAN arm (budget-capped — a denser
        //neighborhood implies a sparser one elsewhere, the average says
        //so) and the WORKABILITY arm (uncapped — density < 5/6
        //strictly keeps the lattice stride ≤ FREE_STRIDE, so every
        //planted hole stays slide-reachable; the stop IS the stride
        //cap). the grow branch serves workability ONLY — `need`
        //underflows against an already-workable span. the whole block
        //always satisfies the mean arm, so the scan terminates. u128:
        //the mean cross-multiply can hit 2^64 at u32 caps
        loop {
            let above_mean = span < budget
                && (live as u128) * (cap as u128) > (self.len as u128) * (span as u128);
            let unworkable = live * FREE_DEN >= span * FREE_NUM;
            if !above_mean && !unworkable {
                break;
            }
            let left = wlo > 0;
            let right = (whi + 1) << 6 < cap;
            //tail-bounded and unworkable: GROW — fresh slots are the
            //cheapest space (nothing relocates, names mint on demand);
            //blocked at the name ceiling (§18): the nearest hole slides,
            //however far
            if unworkable && !right
                && (!left || self.mask.word_at(&self.block, (wlo - 1) << 6).count_ones() > 0)
            {
                let need = live * FREE_DEN / FREE_NUM + 1;
                let grow = need - span;
                if cap + grow > self.slot_max {
                    let hole = match self.mask.nearest_clear(&self.block, spot.0) {
                        Some(hole) => hole,
                        None => {
                            self.push_slot();
                            self.cap() - 1
                        }
                    };
                    return self.slide_open(hole, spot.0);
                }
                self.push_slots(grow);
                cap = self.cap();
                whi = (cap - 1) >> 6;
                span = cap - (wlo << 6);
                continue;
            }
            if !left && !right {
                break; //whole block absorbed, nowhere to extend
            }
            let take_left = match (left, right) {
                (true, true) => {
                    let lw = self.mask.word_at(&self.block, (wlo - 1) << 6).count_ones();
                    let rw = self.mask.word_at(&self.block, (whi + 1) << 6).count_ones();
                    lw <= rw
                }
                (true, false) => true,
                _ => false,
            };
            if take_left {
                wlo -= 1;
                live += self.mask.word_at(&self.block, wlo << 6).count_ones() as usize;
            } else {
                live += self.mask.word_at(&self.block, (whi + 1) << 6).count_ones() as usize;
                whi += 1;
            }
            span = ((whi + 1) << 6).min(cap) - (wlo << 6);
        }
        let base = wlo << 6;
        //clamped: the tail word may extend past cap, and the lattice's
        //positions must stay < cap
        let end = ((whi + 1) << 6).min(cap);
        if live + 1 > span {
            //slack 0 — the scan stopped solid (budget short of any
            //hole): the nearest hole slides, however far; a globally
            //solid mask mints first (len < ceiling leaves room)
            let hole = match self.mask.nearest_clear(&self.block, spot.0) {
                Some(hole) => hole,
                None => {
                    self.push_slot();
                    self.cap() - 1
                }
            };
            return self.slide_open(hole, spot.0);
        }
        if live == 0 {
            //an empty region: k takes the floor, no lattice needed
            self.deltas.record_none_slide(self.translator, Pos(base), Pos(base));
            return Pos(base);
        }
        let region = self.mask.region_mask(&self.block, base, end);
        //k's rank among the region's live — the boundary is count-below
        //in the pre-move mask (order is preserved)
        let r = region.count_below((spot.0.min(end) - base).min(region.bits()));
        //minimal stride whose lattice holds live+1 (k included): u(live) < span
        let mut stride = 2;
        while live + live / (stride - 1) >= span {
            stride += 1;
        }
        let s1 = stride - 1;
        //old-rank i's lattice slot: one past k from `r` up
        let target = |i: usize| i + usize::from(i >= r) + (i + usize::from(i >= r)) / s1 + base;
        debug_assert!(target(live - 1) < end, "the lattice must fit the region");
        //the two passes exactly as the pair emission (step_state's
        //Rebalance): right-movers descending — every target is vacant
        //when written — then left-movers ascending, the mirrored argument
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
        self.deltas.record_rebalance(self.translator, Pos(base), r, stride, region);
        Pos(base + r + r / s1)
    }

    ///rotation turn (in-place): cap doubles, every live element relocates
    ///to `a2p_new(name)` — names preserved by construction. the doubled
    ///range must fit the FIXED mask region (slot_max) — refusal there
    ///escalates to open()'s ceiling valves like the other guards. false ⇒
    ///refused (slot ceiling, name-space midpoint passed, or rotation
    ///budget spent)
    fn grow_rotate(&mut self) -> bool {
        let old_cap = self.cap();
        let Some(new_cap) = old_cap.checked_mul(2) else {
            return false;
        };
        //refusal guards: doubled slots past the fixed mask's coverage;
        //doubled positions would exceed the address space past the
        //midpoint; the rotation knob would wrap; a REVERSED turn needs
        //offset < midpoint (§30 — a cleave-shifted half can carry offset
        //≥ its cap, sound while Asc) — refuse and let open() route the
        //space need to the ceiling valve
        if new_cap > self.slot_max
            || old_cap > A::MIDPOINT.as_usize()
            || self.translator.rotation() >= A::BIT_WIDTH as u32
            || self.translator.reversed()
                && self.translator.offset().as_usize() >= A::MIDPOINT.as_usize()
        {
            return false;
        }
        let old_tr = self.translator;
        let turn_start = Instant::now();
        let new_tr = old_tr.rotate();
        self.grow_data(old_cap);
        //the map is monotone with each target ≥ its source — the
        //descending pass writes every target before its source is read.
        //any offset, either direction: the band (upper-half names land
        //odd) is rotate_mask's job
        let mut anchor_moved = false;
        for p in (0..old_cap).rev() {
            if self.mask.get(&self.block, p) {
                let q = new_tr.a2p(old_tr.p2a(Pos(p)));
                debug_assert!(q.0 >= p, "turn must not move an element down");
                if p == self.last_insert {
                    self.last_insert = q.0;
                    anchor_moved = true;
                }
                let data = self.data_mut();
                data[q.0] = std::mem::replace(&mut data[p], MaybeUninit::uninit());
            }
        }
        self.mask.rotate_mask(
            &mut self.block,
            self.translator.offset().as_usize(),
            A::MIDPOINT.as_usize(),
            self.translator.reversed(),
        );
        self.translator = new_tr;
        //the pos-space doubled — the insert-position sum tracks it
        self.pos_sum = self.pos_sum.wrapping_mul(2);
        //positions spread — a silent move for pos holders. the streak
        //anchor followed its element exactly in the loop; the ×2
        //approximation is only the fallback for a dead (removed) anchor
        if !anchor_moved {
            self.last_insert = self.last_insert.wrapping_mul(2);
        }
        self.epoch += 1;
        TURN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TURN_MOVED.fetch_add(old_cap as u64, std::sync::atomic::Ordering::Relaxed);
        TURN_TIME_NS.fetch_add(turn_start.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        true
    }

    ///extend by one uninit slot (mask clear) — the block grows if the
    ///free gap can't hold it. panics past the slot ceiling
    fn push_slot(&mut self) {
        assert!(self.cap() < self.slot_max, "slot ceiling exhausted");
        self.grow_data(1);
        self.mask.resize(self.cap());
    }

    ///extend by `n` uninit slots (mask clear) in one extend — the
    ///rebalance's tail-growth valve. caller checks the ceiling
    fn push_slots(&mut self, n: usize) {
        assert!(self.cap() + n <= self.slot_max, "slot ceiling exhausted");
        self.grow_data(n);
        self.mask.resize(self.cap());
    }

    ///deferred — see CLAUDE.md Future Work; panics
    pub fn from_vec(v: Vec<K>) -> Self {
        let _ = v;
        todo!()
    }
}

///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr = u32> {
    Asc { bits: SetBitsW<'a>, set: &'a BlockSet<K, A, P> },
    Desc { bits: SetBitsRevW<'a>, set: &'a BlockSet<K, A, P> },
}

impl<'a, K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr> Iterator for Iter<'a, K, A, P> {
    type Item = &'a K::Handle;

    #[inline]
    fn next(&mut self) -> Option<&'a K::Handle> {
        match self {
            //mask ⇔ init invariant upholds the read
            Iter::Asc { bits, set } => bits.next().map(|p| unsafe { set.key(p) }),
            Iter::Desc { bits, set } => bits.next().map(|p| unsafe { set.key(p) }),
        }
    }
}

///key-ordered bidirectional walk — `seek`/`seek_after` position, `next`
///walks key-ascending (yield current, then step), `prev` walks back (step,
///then yield). stepping runs on `SlotCursor` (the cached-word walk over the
///block-backed mask). holds raw slots: any mutation (slides, turns, flips)
///invalidates it — re-seek
pub struct Cursor<'a, K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr = u32> {
    set:  &'a BlockSet<K, A, P>,
    ///the slot-walk machine; dir = !reversed, baked at construction
    walk: SlotCursor,
}

impl<'a, K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr> Cursor<'a, K, A, P> {
    fn new(set: &'a BlockSet<K, A, P>) -> Self {
        //dir: positions ascend — unless the block is flipped
        Cursor { set, walk: SlotCursor::start(!set.translator.reversed()) }
    }
}

impl<'a, K: Archive<P> + RelOrd<P>, A: UnsignedNum, P: Ptr> Cursor<'a, K, A, P> {
    ///reposition at the first key ≥ k
    pub fn seek(&mut self, k: &K) {
        match self.set.bsearch(k) {
            Ok(p) => self.walk.at(&self.set.mask, &self.set.block, p.0),
            Err(spot) => match self.set.key_bound(spot.0) {
                Some(q) => self.walk.at(&self.set.mask, &self.set.block, q),
                None => self.walk.end(),
            },
        }
    }

    ///reposition at the first key > k (a dup at k skipped)
    pub fn seek_after(&mut self, k: &K) {
        let q = match self.set.bsearch(k) {
            //past the dup in key order — dir = Asc: forward skip
            Ok(p) => {
                if self.walk.dir() {
                    self.set.mask.next_set(&self.set.block, p.0 + 1)
                } else {
                    self.set.mask.prev_set(&self.set.block, p.0)
                }
            }
            Err(spot) => self.set.key_bound(spot.0),
        };
        if let Some(q) = q {
            self.walk.at(&self.set.mask, &self.set.block, q)
        } else {
            self.walk.end()
        }
    }

    ///yield the current element, step forward (key-ascending). None at
    ///the walk's end.
    //#[inline(always)]: plain #[inline] left next outlined (both
    //directions' crossing scans inline into it, blowing LLVM's size
    //budget) and walk loops paid a call + cursor spill per step —
    ///measured 2× @10k fwd
    #[inline(always)]
    pub fn next(&mut self) -> Option<&'a K::Handle> {
        let p = self.walk.slot()?;
        let v = unsafe { self.set.key(p) };
        self.walk.advance(&self.set.mask, &self.set.block, p);
        Some(v)
    }

    ///step back (key-descending), yield the element now current. None at
    ///the walk's start (the state stays at the first element — next
    ///re-yields it); from the forward ghost, resumes at the last element
    #[inline(always)]
    pub fn prev(&mut self) -> Option<&'a K::Handle> {
        let q = self.walk.retreat(&self.set.mask, &self.set.block)?;
        Some(unsafe { self.set.key(q) })
    }

    ///the element next() would yield, without moving
    pub fn key(&self) -> Option<&'a K::Handle> {
        self.walk.slot().map(|p| unsafe { self.set.key(p) })
    }

    ///the current element's name
    pub fn addr(&self) -> Option<A> {
        self.walk.slot().map(|p| self.set.translator.p2a(Pos(p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///apply translated deltas as name swaps, in the given order; returns
    ///the pair count (assert-and-apply in one pull — the drain IS the
    ///application)
    fn apply<A: UnsignedNum>(holder: &mut Vec<(A, u64)>, deltas: &mut Deltas<A>) -> usize {
        let mut pairs = 0;
        for (from, to) in deltas.iter() {
            pairs += 1;
            for (a, _) in &mut *holder {
                if *a == from {
                    *a = to;
                    break;
                }
            }
        }
        pairs
    }

    fn insert_all(keys: impl IntoIterator<Item = u64>) -> (BlockSet<u64, u8>, Vec<(u8, u64)>) {
        let mut set = BlockSet::new();
        let mut holder = Vec::new();
        for k in keys {
            match set.insert(k) {
                Ok(addr) => {
                    apply(&mut holder, set.deltas());
                    holder.push((addr, k));
                }
                Err(k) => panic!("insert of {k} failed"),
            }
        }
        (set, holder)
    }

    fn check<A: UnsignedNum>(set: &BlockSet<u64, A>, holder: &[(A, u64)], count: usize) {
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
            assert_eq!(set.get(*a), Some(k), "holder name {:?} -> key {k} broken", a);
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
                match set.insert(k) {
                    Ok(a) => {
                        apply(&mut holder, set.deltas());
                        holder.push((a, k));
                    }
                    Err(_) => set.deltas().clear(),
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
        for k in (0..100u64).map(|i| i * 2) {
            set.insert(k).unwrap();
            set.deltas().clear();
        }
        assert!(set.cap() < 128, "cap {} — turned instead of pushing back", set.cap());
        assert_eq!(set.cap() - set.len(), 0, "appends left holes");
        set.insert(101).unwrap();
        set.deltas().clear();
        assert_eq!(set.cap(), 200, "mid insert into full mask must turn");
        set.insert(103).unwrap();
        set.deltas().clear();
        assert_eq!(set.cap(), 200, "second mid insert must not turn");
        let keys: Vec<u64> = set.iter().copied().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(set.len(), 102);
    }

    ///the ceiling regime (cap == name space): a descending fill strands
    ///turn-minted holes past the push edge; the last inserts must BANK
    ///them in one gather (the len/2 gate's mint premise is void at the
    ///ceiling), not pay a same-length slide per remaining hole
    #[test]
    fn ceiling_banks_stranded_holes() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut moved = 0usize;
        for k in (0..256u64).rev() {
            set.insert(k).unwrap();
            moved += set.deltas().iter().count();
        }
        assert_eq!(set.len(), 256);
        assert_eq!(set.cap(), 256);
        let keys: Vec<u64> = set.iter().copied().collect();
        assert_eq!(keys, (0..256).collect::<Vec<_>>());
        //one bank pass ≈ one run; the pre-fix slide chain moved ~7000
        assert!(moved < 1000, "ceiling paid {moved} moves");
    }

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
    fn streak_anchor_staleness() {
        let track = |name: &str, keys: Vec<u64>| {
            let mut set = BlockSet::<u64, u16>::new();
            let mut prev: Option<u16> = None;
            let (mut stale, mut resets) = (0usize, 0);
            for k in keys {
                let anchor = set.last_insert;
                let streak_before = set.streak;
                let epoch = set.epoch();
                let Ok(a) = set.insert(k) else {
                    set.deltas().clear();
                    continue; //dup — nothing landed, prev stays live
                };
                resets += usize::from(set.streak == 0 && streak_before > 0);
                if let Some(mut prev_name) = prev
                    && set.epoch() == epoch
                {
                    //one pass over the event: replay the pos pairs onto
                    //the anchor (the expected tracking) AND apply the
                    //name swaps to our held name (honest-holder
                    //semantics — names transfer on slides)
                    let tr = set.deltas().translator;
                    let mut expected = anchor;
                    while let Some((f, t)) = set.deltas().next_pos() {
                        if f.0 == expected {
                            expected = t.0;
                        }
                        if tr.p2a(f) == prev_name {
                            prev_name = tr.p2a(t);
                        }
                    }
                    let true_pos = set.pos_of(prev_name);
                    if true_pos != expected && stale < 6 {
                        println!(
                            "{name} MISMATCH anchor {anchor} expected {expected} true {true_pos} k={k}",
                        );
                    }
                    if true_pos != expected {
                        stale += 1;
                    }
                }
                set.deltas().clear();
                prev = Some(a);
            }
            println!(
                "{name:>12}: stale {stale:>4} | streak resets {resets:>3} / {}",
                set.len(),
            );
        };
        //the mid-band shapes from benches/mid_band.rs, plus clustered random
        let base: Vec<u64> = (0..128).map(|i| i as u64 * 8).collect();
        track("mid desc", base.iter().copied().chain((0..128u64).rev().map(|i| 258 + i * 4)).collect());
        track("mid asc", base.iter().copied().chain((0..128u64).map(|i| 258 + i * 4)).collect());
        track("clust rand", {
            let mut s = 0x2545_F491_4F6C_DD1Du64;
            let mut rnd = move || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s
            };
            (0..600u64).map(|i| (i / 40) * 700 + rnd() % 300).collect()
        });
        track("random", {
            let mut s = 0x2545_F491_4F6C_DD1Du64;
            let mut rnd = move || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s
            };
            (0..600u64).map(|_| rnd() % 1200).collect()
        });
    }

    ///the hinted bracket must agree with the bsearch everywhere it
    ///answers: Some(Ok) = the same boundary position, Some(Err) = the
    ///same dup. holed blocks in both directions, probed at every key
    #[test]
    fn spot_near_agrees_with_bsearch() {
        let mut set = BlockSet::<u64, u16>::new();
        for i in 0..120u64 {
            set.insert((i * 37 + 11) % 131).unwrap();
            set.deltas().clear();
        }
        for k in [11u64, 48, 18, 59] {
            set.remove(set.find(&k).unwrap());
        }
        let check = |set: &BlockSet<u64, u16>| {
            for k in 0..=132u64 {
                match set.spot_near(&k) {
                    Some(Ok(p)) => {
                        let b = set.bsearch(&k);
                        assert_eq!(b.err(), Some(p), "k {k} boundary mismatch");
                    }
                    Some(Err(())) => {
                        assert!(set.bsearch(&k).is_ok(), "k {k} false dup");
                    }
                    None => {} //out of reach — bsearch carries it
                }
            }
        };
        check(&set);
        set.reverse(); //Desc — the walk mirrors
        check(&set);
    }

    ///one-off diagnostic: the mid-band bench's streak health — resets
    ///(a landing > FREE_STRIDE from the anchor) vs hot inserts (a live
    ///streak) per band, isolated on fresh prefills like the bench's
    ///independent rows. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_streak_census() {
        let track = |name: &str, keys: Vec<u64>| {
            let mut set = BlockSet::<u64, u16>::new();
            let (mut resets, mut hot, mut cold) = (0usize, 0, 0);
            for k in keys {
                let before = set.streak;
                set.insert(k).unwrap();
                set.deltas().clear();
                resets += usize::from(set.streak == 0 && before > 0);
                hot += usize::from(set.streak > 0);
                cold += usize::from(set.streak == 0);
            }
            println!("{name:>14}: resets {resets:>3} | hot {hot:>3} | cold {cold:>3} / {n}", n = set.len());
        };
        let base: Vec<u64> = (0..128).map(|i| i as u64 * 8).collect();
        let prefill = |track: &dyn Fn(&str, Vec<u64>), band: Vec<u64>, name: &str| {
            let mut keys = base.clone();
            keys.extend(band);
            track(name, keys);
        };
        prefill(&track, (0..128u64).rev().map(|i| 258 + i * 4).collect(), "desc band iso");
        prefill(&track, (0..128u64).map(|i| 385 + i * 2).collect(), "asc band iso");
        let mut nested = base.clone();
        nested.extend((0..128u64).rev().map(|i| 258 + i * 4));
        nested.extend((0..128u64).map(|i| 385 + i * 2));
        track("nested both", nested);
    }

    ///one-off diagnostic: the exact anatomy of each streak break in the
    ///mid-band workloads — insert index, landing vs anchor, the event
    ///class that fired, and the anchor's true post-event position (a
    ///stale anchor names a false break). also logs turns/rebalances for
    ///correlation. run with --release --nocapture. NOT API
    #[test]
    fn mid_band_break_forensics() {
        let track = |name: &str, keys: Vec<u64>| {
            let mut set = BlockSet::<u64, u16>::new();
            
            for (i, k) in keys.into_iter().enumerate() {
                let anchor = set.last_insert;
                let epoch = set.epoch();
                set.insert(k).unwrap();
                let links: Vec<_> = set.deltas().slides().collect();
                let moved = set.deltas().iter().count();
                let kind = if set.epoch() != epoch {
                    "TURN/FLIP"
                } else if links.is_empty() || links[0].from == links[0].to {
                    "direct"
                } else if links.len() > 1 {
                    "bank"
                } else if moved == links[0].from.0.abs_diff(links[0].to.0) {
                    "slide"
                } else {
                    "REBALANCE"
                };
                let drift = set.last_insert.abs_diff(anchor);
                //true_prev resolves a name this test never applies swaps
                //to — the honest-holder check lives in
                //streak_anchor_staleness (0 mismatches); this line only
                //flags silent moves (turns/flips), which preserve names
                let trace = set.streak == 0
                    || drift > FREE_STRIDE
                    || matches!(kind, "TURN/FLIP" | "REBALANCE" | "bank");
                if trace {
                    println!(
                        "{name} i={i:>3} k={k:>4}: landed {land:>4} Δ{drift:>3} {kind:>8} moved {moved:>3} streak {streak:>2} cap {cap}",
                        land = set.last_insert, streak = set.streak, cap = set.cap(),
                    );
                }
                set.deltas().clear();

            }
        };
        let base: Vec<u64> = (0..128).map(|i| i as u64 * 8).collect();
        let mut desc = base.clone();
        desc.extend((0..128u64).rev().map(|i| 258 + i * 4));
        track("desc", desc);
        let mut asc = base.clone();
        asc.extend((0..128u64).map(|i| 385 + i * 2));
        track("asc", asc);
        //the runs shape (chunked, alternating reversed) — bank-to-bank
        //gaps: how many inserts does one bank actually serve?
        let mut runs_keys: Vec<u64> = Vec::new();
        let mut chunks: Vec<Vec<u64>> = (0..50)
            .map(|c| ((c * 300) as u64..((c * 300) as u64 + 200)).collect())
            .collect();
        for c in chunks.iter_mut().step_by(2) {
            c.reverse();
        }
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        for i in (1..chunks.len()).rev() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            chunks.swap(i, (s % (i as u64 + 1)) as usize);
        }
        runs_keys.extend(chunks.into_iter().flatten());
        let mut set = BlockSet::<u64, u32>::new();
        let mut last_bank = 0usize;
        let mut banks = 0usize;
        for (i, k) in runs_keys.into_iter().enumerate() {
            set.insert(k).unwrap();
            let links: Vec<_> = set.deltas().slides().collect();
            let moved = set.deltas().iter().count();
            set.deltas().clear();
            if links.len() > 1 {
                banks += 1;
                if banks <= 24 {
                    println!(
                        "runs bank #{banks} i={i} gap {} moved {moved} streak {}",
                        i - last_bank,
                        set.streak,
                    );
                }
                last_bank = i;
            }
        }
        println!("runs total banks {banks} / {}", set.len());
    }

    ///the ceiling regime (cap > the doubling midpoint, turns refused):
    ///a solid mask MINTS tail slots (want = 1 + dist/FREE_STRIDE,
    ///clamped to the name ceiling) and banks them — the 99-element run
    ///shifts once, and the banked holes absorb the following mid
    ///inserts with 1-pair slides instead of the O(n) grind
    #[test]
    fn ceiling_mints_tail_and_banks() {
        let mut set = BlockSet::<u64, u8>::new();
        for k in (0..200u64).map(|i| i * 2) {
            set.insert(k).unwrap();
            set.deltas().clear();
        }
        assert_eq!(set.cap(), 200);
        assert_eq!(set.cap() - set.len(), 0, "appends stay dense");
        //spot 101 (key 201 between 200@100 and 202@101): turn line
        //holds but doubling is refused (cap 200 > MIDPOINT 128) — the
        //mint+bank valve fires instead
        set.insert(201).unwrap();
        assert_eq!(set.cap(), 217, "minted 17, banked them all");
        assert_eq!(set.get(101), Some(&201));
        set.deltas().clear();
        //the hot bank strided its landings: one sentinel splits the
        //banked range, keeping 202 adjacent to it — the followers
        //subdivide/slide into the strided holes: 0 + 2 + 5 pairs
        let mut moved = 0usize;
        for k in [203u64, 205, 207] {
            set.insert(k).unwrap();
            moved += set.deltas().iter().count();
        }
        assert_eq!(moved, 7, "the sentinel-split slide chain");
        assert_eq!(set.cap(), 217, "no second mint");
        let keys: Vec<u64> = set.iter().copied().collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
        assert_eq!(set.len(), 204);
    }

    ///BANK: the nearest hole 40 away — want = 40/FREE_STRIDE = 6 holes
    ///banked beside the boundary, the 40-element run shifts once, and
    ///the banked range absorbs the next insert with a 1-pair slide; no
    ///turn either time
    #[test]
    fn bank_absorbs_followers() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut holder = Vec::new();
        //words 0-1 dense (80 keys), then word 1's tail holed by removals
        for k in (0..80u64).map(|i| i * 2) {
            let a = set.insert(k).unwrap();
            apply(&mut holder, set.deltas());
            holder.push((a, k));
        }
        //positions 66..79 clear — word 1 at 2/16 valid, well under
        //High; len 66 keeps the block under the 5/6 turn line
        for k in (66..80u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        let a = set.insert(51).unwrap();
        //spot 26 (key 51 between 50@25 and 52@26): one event banks 6 of
        //word 1's holes — the 40-element run [26, 66) shifts up, k takes
        //26 (the dead anchor sits 53 out — slack 0)
        assert_eq!(set.cap(), 80, "the bank must not turn");
        assert_eq!(set.get(26), Some(&51));
        assert_eq!(apply(&mut holder, set.deltas()), 40, "bank pairs");
        holder.push((a, 51));
        //len back under the 5/6 turn line, then the bank absorbs the
        //next insert: key 53 slides ONE element from a banked hole
        set.remove(set.find(&130).unwrap());
        holder.retain(|(_, hk)| *hk != 130);
        let a = set.insert(53).unwrap();
        assert_eq!(set.cap(), 80, "no turn — the banked hole absorbs");
        assert_eq!(apply(&mut holder, set.deltas()), 1, "banked hole should absorb the insert");
        holder.push((a, 53));
        check(&set, &holder, 67);
    }

    ///FAR: the nearest hole 102 away (both flanking words solid) — a
    ///distance-proportional bank: want = 102/FREE_STRIDE = 17 holes
    ///COLD FAR: the nearest hole 102 away, streak dead — the
    ///water-leveling arm scans to the block mean (the emptied word 2
    ///drops it exactly there), redistributes onto the stride-4 lattice
    ///with k riding at its rank, no turn, no growth
    #[test]
    fn far_hole_water_levels() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut holder = Vec::new();
        //190 keys dense: words 0-1 full, word 2 holds 62 of 64 bits
        for k in (0..190u64).map(|i| i * 2) {
            let a = set.insert(k).unwrap();
            apply(&mut holder, set.deltas());
            holder.push((a, k));
        }
        //word 2 emptied — 62 holes 102 past the probe's boundary
        for k in (128..190u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        //a below-tail probe fills the emptied word's gap at its head —
        //zero movement
        let a = set.insert(261).unwrap();
        assert_eq!(apply(&mut holder, set.deltas()), 0, "direct place, no movement");
        holder.push((a, 261));
        set.remove(a);
        holder.retain(|(_, hk)| *hk != 261);
        let a = set.insert(51).unwrap();
        //spot 26 (key 51 between 50@25 and 52@26): nearest hole 128 —
        //streak dead, the cold arm water-levels; the region is the whole
        //block (mean reached exactly), stride 4, k at u(26) = 34
        assert_eq!(set.cap(), 190, "no turn, no growth");
        assert_eq!(set.get(34), Some(&51), "k's lattice slot");
        assert_eq!(apply(&mut holder, set.deltas()), 125, "rebalance pairs");
        holder.push((a, 51));
        check(&set, &holder, 129);
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

    ///STRIDED BANK, direct drive: compact_gather + record_gather with 49
    ///holes — a pinned unit test of the sentinel machinery, driven
    ///directly because the ladder only reaches drag ≥ 1 on a hot
    ///streak with dist ≥ 96. every
    ///aligned 16-window across the banked range holds ≥1 live (the skip
    ///list's stride window), and the pair expansion retargets every
    ///holder exactly once through the sentinel moves
    #[test]
    fn strided_bank_sentinels_direct() {
        let mut set = BlockSet::<u64, u16>::new();
        let mut holder = Vec::new();
        for k in (0..300u64).map(|i| i * 2) {
            let a = set.insert(k).unwrap();
            apply(&mut holder, set.deltas());
            holder.push((a, k));
        }
        //49 holes at the tail: positions 251..300
        for k in (251..300).map(|p| p as u64 * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        assert_eq!(set.len(), 251);
        //a 49-hole right bank at spot 20 — drag = 3 sentinels
        let holes: Vec<usize> = (251..300).collect();
        set.compact_gather(Pos(20), 20, true, &holes);
        set.deltas.record_gather(set.translator(), 20, true, &holes);
        //the run's head elements ride in as sentinels — one live per 16
        assert_eq!(set.slot(35), Some(40));
        assert_eq!(set.slot(51), Some(42));
        assert_eq!(set.slot(67), Some(44));
        //every aligned stride window touching the banked range [20, 72)
        //holds a live item
        for lo in (16..80).step_by(16) {
            assert!(
                set.first_live_pos(lo).is_some_and(|p| p < lo + 16),
                "stride window [{lo}, {}) hole-only",
                lo + 16
            );
        }
        //228 shifted elements + 3 sentinels — one pair each
        assert_eq!(apply(&mut holder, set.deltas()), 231);
        check(&set, &holder, 251);
    }

    ///STRIDED BANK, LEFT mirror: the same 49-hole drive fetched from
    ///below — the sentinel landings mirror (open − (16i+15)), the run
    ///shifts down, and every aligned 16-window across the banked range
    ///still holds ≥1 live
    #[test]
    fn strided_bank_sentinels_left_direct() {
        let mut set = BlockSet::<u64, u16>::new();
        let mut holder = Vec::new();
        for k in (0..300u64).map(|i| i * 2) {
            let a = set.insert(k).unwrap();
            apply(&mut holder, set.deltas());
            holder.push((a, k));
        }
        //49 holes at the head: positions 0..49
        for k in (0..49).map(|p| p as u64 * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        assert_eq!(set.len(), 251);
        //a 49-hole left bank at spot 250, open 249 — drag = 3 sentinels
        let holes: Vec<usize> = (0..49).rev().collect();
        set.compact_gather(Pos(250), 249, false, &holes);
        set.deltas.record_gather(set.translator(), 249, false, &holes);
        assert_eq!(set.slot(234), Some(498));
        assert_eq!(set.slot(218), Some(496));
        assert_eq!(set.slot(202), Some(494));
        for lo in (192..250).step_by(16) {
            assert!(
                set.first_live_pos(lo).is_some_and(|p| p < lo + 16),
                "stride window [{lo}, {}) hole-only",
                lo + 16
            );
        }
        //198 shifted elements + 3 sentinels — one pair each
        assert_eq!(apply(&mut holder, set.deltas()), 201);
        check(&set, &holder, 251);
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
            let (mut slides, mut s_moved, mut rebal, mut r_span, mut r_moved, mut gath, mut g_moved, mut turns) =
                (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
            let mut epoch = set.epoch();
            for k in keys {
                set.insert(k).unwrap();
                if set.epoch() != epoch {
                    epoch = set.epoch();
                    turns += 1;
                }
                //slides read first — the pair pull drains the event
                let links: Vec<_> = set.deltas().slides().collect();
                let pairs = set.deltas().iter().count() as u64;
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

    ///LEFT BANK: head holes 102 below the boundary — the bank fetches
    ///COLD LEFT: the nearest hole 102 below, streak dead — the scan
    ///absorbs the solid tail-side words, still unworkable, and GROWS
    ///the tail to fit the lattice (cap 200 → 228); k rides at its
    ///rank, no turn
    #[test]
    fn cold_left_grows_unworkable_tail() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut holder = Vec::new();
        //200 dense: word 3 = the 8-valid tail word; then holes at the
        //HEAD (global 75% ≤ the turn line, so no preemptive turn)
        for k in (0..200u64).map(|i| i * 2) {
            let a = set.insert(k).unwrap();
            apply(&mut holder, set.deltas());
            holder.push((a, k));
        }
        for k in (0..50u64).map(|i| i * 2) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        //a below-min probe subdivides the head gap at its middle —
        //zero movement
        let a = set.insert(1).unwrap();
        assert_eq!(apply(&mut holder, set.deltas()), 0, "direct place, no movement");
        holder.push((a, 1));
        let epoch = set.epoch();
        let a = set.insert(301).unwrap();
        //spot 151 (key 301 between 300@150 and 302@151): no hole above,
        //nearest below at 49 — the cold arm grows the tail 28 to fit
        //the lattice, k lands at its rank slot
        assert_eq!(set.cap(), 228, "the unworkable tail region grows");
        assert_eq!(set.epoch(), epoch, "no turn");
        assert_eq!(set.get(153), Some(&301), "k's lattice slot");
        assert_eq!(apply(&mut holder, set.deltas()), 149, "rebalance pairs");
        holder.push((a, 301));
        check(&set, &holder, 152);
    }

    ///front insert below cap: plain slide, name transfer without any turn —
    ///the run 20,10 cascades right (hole-adjacent first), 5 takes name 0
    #[test]
    fn front_insert_renames_min() {
        let mut set = BlockSet::<u64, u8>::new();
        set.insert(10).unwrap();
        set.deltas().clear();
        set.insert(20).unwrap();
        set.deltas().clear();
        let a = set.insert(5).unwrap();
        assert_eq!(a, 0);
        assert_eq!(set.deltas().iter().collect::<Vec<_>>(), vec![(1, 2), (0, 1)]);
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
        for k in [10, 20, 30, 40] {
            set.insert(k).unwrap();
            set.deltas().clear();
        }
        let a = set.insert(5).unwrap();
        assert_eq!(a, 0);
        assert_eq!(set.deltas().iter().collect::<Vec<_>>(), vec![(3, 4), (2, 3), (1, 2), (0, 1)]);
        assert_eq!(set.get(0), Some(&5));
        assert_eq!(set.get(1), Some(&10));
        assert_eq!(set.get(4), Some(&40));
        assert_eq!(set.cap(), 5);
    }

    #[test]
    fn duplicates_rejected() {
        let mut set = BlockSet::<u64, u8>::new();
        set.insert(1).unwrap();
        assert!(set.insert(1).is_err());
        assert_eq!(set.len(), 1);
    }

    ///the compact NoneSlide event expands to the same cascade the old
    ///per-element pairs produced, both directions
    #[test]
    fn none_slide_expands_to_cascade() {
        let expand = |from: usize, to: usize| -> Vec<(Pos, Pos)> {
            let mut d = Deltas::<u8>::new();
            d.record_none_slide(Translator::new(), Pos(from), Pos(to));
            std::iter::from_fn(|| d.next_pos()).collect()
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
        set.insert(1).unwrap();
        set.insert(2).unwrap();
        set.insert(3).unwrap();
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

    ///alternating front chunks (descending-in-Asc and ascending-in-Desc
    ///both land at the front) with tail filler keeping the all-time
    ///insert mean high — the mean trigger must hold the block steady
    ///instead of flip-flopping. NOTE: the z-gate that once gated this
    ///workload died with the 2026-10-23 trigger swap; this pins the
    ///mean side only — alternating front streams with NO tail filler
    ///are ungated (the mean never clears log2(len))
    #[test]
    fn reversal_trigger_holds_on_alternating_chunks() {
        let mut set = BlockSet::<u64, u16>::new();
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
                set.insert(k).unwrap();
                set.deltas().clear();
                if set.translator.reversed() != prev {
                    prev = set.translator.reversed();
                    flips += 1;
                }
            }
            //tail filler keeps len growing and the mean bouncing — high
            //disjoint key range, so every filler lands at the tail
            for k in (base + 200..base + 400).map(|i| 1_000_000 + i * 3) {
                set.insert(k).unwrap();
                set.deltas().clear();
                if set.translator.reversed() != prev {
                    prev = set.translator.reversed();
                    flips += 1;
                }
            }
        }
        assert!(flips <= 2, "the mean gate must not thrash — {flips} flips");
    }

    #[test]
    fn remove_and_reinsert() {
        let mut set = BlockSet::<u64, u8>::new();
        for k in 0..50u64 {
            set.insert(k).unwrap();
            set.deltas().clear();
        }
        let a = set.find(&25).unwrap();
        assert_eq!(set.remove(a), Some(25));
        assert_eq!(set.remove(a), None);
        set.insert(25).unwrap();
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
        //37 coprime 67 ⇒ distinct keys, 7 gaps for hole spots
        for i in 0..60u64 {
            let k = (i * 37 + 11) % 67;
            set.insert(k).unwrap();
            set.deltas().clear();
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
        set.insert(7).unwrap();
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

    ///NARROW POINTER: P = u16 (64 KB block ceiling) with an explicit slot
    ///ceiling — turns double inside the fixed mask's coverage, the fixture
    ///mirror + key order hold, and the mask region never grows past its
    ///init words (the port's mask-fixed invariant)
    #[test]
    fn narrow_pointer_width_fixed_mask() {
        let mut set = BlockSet::<u64, u16, u16>::with_capacity(64);
        let words0 = set.block.stack_buf::<u64>(set.mask.s).len();
        //u16-P — the u32-P check()/apply helpers don't fit; the mirror runs inline
        let mut holder = Vec::new();
        for i in 0..64u64 {
            let k = i * 45 % 199; //45 invertible mod 199 — 64 distinct keys
            let a = set.insert(k).unwrap();
            for (from, to) in set.deltas().iter() {
                for (a, _) in &mut *holder {
                    if *a == from {
                        *a = to;
                        break;
                    }
                }
            }
            holder.push((a, k));
        }
        assert_eq!(holder.len(), 64);
        let mut prev = None;
        for k in set.iter() {
            if let Some(p) = prev {
                assert!(*k > p, "order broken");
            }
            prev = Some(*k);
        }
        for (a, k) in &holder {
            assert_eq!(set.get(*a), Some(k), "holder name {a:?} -> key {k} broken");
        }
        assert!(set.cap() <= set.slot_max);
        assert_eq!(set.mask.bits, set.cap(), "mask range tracks cap");
        assert_eq!(
            set.block.stack_buf::<u64>(set.mask.s).len(),
            words0,
            "the mask region never grows after init"
        );
    }

    ///FAILED ARCHIVE: a heap key past the block's byte width leaves BOTH
    ///fronts untouched (stack front + occupied byte sum identical, the
    ///heap front rewound) and the holder mirror survives the Err intact
    #[test]
    fn failed_insert_leaves_fronts_unchanged() {
        let mut set = BlockSet::<String, u16, u16>::with_capacity(64);
        let mut holder: Vec<(u16, u64)> = Vec::new();
        let key = |i: usize| format!("k{i:0>1999}"); //2000 bytes — heap keys
        //lexicographic ascending → appends; the block grows for both fronts
        //until the 64 KB pointer ceiling bites the heap
        for i in 0..62usize {
            match set.insert(key(i)) {
                Ok(a) => {
                    apply(&mut holder, set.deltas());
                    holder.push((a, i as u64));
                }
                Err(_) => set.deltas().clear(),
            }
        }
        assert!(!holder.is_empty(), "the successes must precede the ceiling");
        //fronts before the failing insert
        let stack0 = set.block.stack_len();
        let used0 = stack0 + set.block.heap_len();
        let big = key(999);
        match set.insert(big.clone()) {
            Ok(_) => panic!("the width ceiling should have refused key {big}"),
            Err(k) => assert_eq!(k, big),
        }
        assert_eq!(set.block.stack_len(), stack0, "stack front untouched");
        assert_eq!(
            set.block.stack_len() + set.block.heap_len(),
            used0,
            "occupancy unchanged (heap front rewound)"
        );
        assert!(!set.deltas().is_pending(), "no event for a failed archive");
        //every earlier key still resolves under its name
        let heap = Heap::new(&set.block);
        for &(a, i) in &holder {
            let expect = key(i as usize);
            let h = match set.get(a) {
                Some(h) => h,
                None => panic!("name {a} lost"),
            };
            assert_eq!(
                String::rel_cmp(h, &expect, &heap),
                Ordering::Equal,
                "key {i} broken"
            );
        }
    }

    ///CLEAVE HEAP REBASE: heap keys (14 bytes — past the inline cutoff)
    ///across a split — every right-half handle resolves through the NEW
    ///half's Heap (the verbatim tail copy reproduces each end-relative
    ///offset) and keeps key order; the left half resolves through its own
    ///untouched heap, and split names survive (position ≥ split_pos ⇒ new
    ///block id, same name)
    #[test]
    fn cleave_rebases_heap_handles() {
        let mut set = BlockSet::<String, u16, u32>::with_capacity(64);
        let mut names: Vec<(u16, String)> = Vec::new();
        for i in 0..30u16 {
            let k = format!("key-{i:0>10}"); //14 bytes — heap-stored
            let a = set.insert(k.clone()).unwrap();
            set.deltas().clear();
            names.push((a, k));
        }
        //ascending stream — names == positions (offset 0, rotation 0)
        let right = set.cleave(15);
        let heap_l = Heap::new(&set.block);
        let heap_r = Heap::new(&right.block);
        assert_eq!(right.block.heap_len(), set.block.heap_len(), "verbatim heap copy");
        for (name, k) in &names {
            let side = if *name >= 15 { &right } else { &set };
            let heap = if *name >= 15 { &heap_r } else { &heap_l };
            let h = side
                .get(*name)
                .unwrap_or_else(|| panic!("split name {name} lost"));
            assert_eq!(String::rel_cmp(h, k, heap), Ordering::Equal, "key {k}");
        }
        //each half's stored keys still ascend under ITS OWN heap
        for w in names.windows(2) {
            let (n0, _) = w[0];
            let (n1, _) = w[1];
            if (n0 >= 15) != (n1 >= 15) {
                continue; //across the split — per-half order only
            }
            let (h0, h1, heap) = if n0 >= 15 {
                (right.get(n0).unwrap(), right.get(n1).unwrap(), &heap_r)
            } else {
                (set.get(n0).unwrap(), set.get(n1).unwrap(), &heap_l)
            };
            assert_eq!(
                String::rel_cmp_h(h0, h1, heap),
                Ordering::Less,
                "half order {n0} {n1}"
            );
        }
        //the source (left) half dropped the moved slots — its cap/len shrank
        assert_eq!(set.len(), 15);
        assert_eq!(right.len(), 15);
    }
}
