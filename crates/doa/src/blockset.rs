//!BlockSet: pos-ordered slot array + 2-knob translator. addr = canonical
//!name of its slot (`p2a`), semi-stable — rotation turns (growth) preserve
//!names, slides transfer them (the mover adopts the vacated name). moves
//!are reported as pos pairs in swap-cascade order (hole-adjacent mover
//!first) + a translator snapshot; the holder applies `Deltas::iter`'s
//!(from_addr, to_addr) name swaps in order.
//!order: pos order == key order — pos 0 = min (Asc) or max (Desc); a
//!sequential insert at the key extreme is always a physical tail
//!push_back. the translator carries the direction (see translator.rs).
//!cursor (`seek`/`seek_after`): bidirectional key-ordered walk over raw
//!slots — invalidated by any mutation, re-seek.

use std::cmp::Ordering;
use std::mem::MaybeUninit;

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
use crate::index::*;
use crate::mask::{Mask, SetBits, SetBitsRev};

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

///swap-cascade run state of one hole slide `from → to`
fn run_of(from: usize, to: usize) -> (usize, i64, usize) {
    if from > to {
        (from - 1, -1, from - to)
    } else {
        (from + 1, 1, to - from)
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
}

///fetch distance per banked hole: a gather fetches one hole per FREE_STRIDE
///of distance to the nearest one (distance 0..=FREE_STRIDE ⇒ 1, etc.)
///preemptive turn line (numerator/denominator): past this density, a
///mid-block insert turns instead of gathering — past the line, the
///expected fetch distance blows through FREE_STRIDE (the stride banking
///assumption dies). 3/4 measured 2026-10-15: −18% random @10k but 2.25×
///WORSE @1M (turn frequency doubles; each big turn's O(cap) relocation
///eats the amortization) + 2× random-build memory — 7/8 stands
pub(crate) const TURN_NUM: usize = 7;
pub(crate) const TURN_DEN: usize = 8;
pub(crate) const FREE_STRIDE: usize = 8;

///~L2 residency: the bsearch shape split — cache-resident blocks cmov
///(probe compares resolve from L1/L2, mispredicts dominate), DRAM-
///resident blocks speculate (the branchy loop issues the next probe's
///load ahead of the compare, overlapping the miss). measured both ways
///at both scales, see notes/session_history.md
pub(crate) const L2_BYTES: usize = 512 * 1024;


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

impl<K, A> Drop for BlockSet<K, A> {
    fn drop(&mut self) {
        for p in self.mask.iter_set() {
            //mask ⇔ init invariant
            unsafe { self.data[p].assume_init_drop() };
        }
    }
}

impl<K: Ord, A: UnsignedNum> BlockSet<K, A> {
    pub fn new() -> Self {
        BlockSet {
            translator: Translator::new(),
            data:      Vec::new(),
            mask:      Mask::new(),
            len:       0,
            pos_sum:   0,
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

    ///None if addr names no live element (never-minted names can't alias a
    ///live slot — a2p is injective; STALE pre-delta names can, see
    ///subtle_bugs §17)
    pub fn get(&self, addr: A) -> Option<&K> {
        let p = self.translator.a2p(addr);
        (p.0 < self.data.len() && self.mask.get(p.0))
            .then(|| unsafe { self.data[p.0].assume_init_ref() })
    }

    #[inline]
    pub fn find(&self, k: &K) -> Option<A> {
        //beyond either extreme → absent, no bsearch (see blockmap::find)
        if self.len > 0 {
            let tail = self.key_tail();
            let beyond = if self.translator.reversed() { *k < *tail } else { *tail < *k };
            if beyond {
                return None;
            }
            //len > 0 ⇒ a first live slot exists
            let head = unsafe { self.key(self.mask.next_set(0).unwrap()) };
            let before = if self.translator.reversed() { *head < *k } else { *k < *head };
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
            return Ok(self.insert_first(k));
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
        //sequential at the back: extend instead of sliding
        if spot.0 == self.data.len() && self.data.len() < Self::max_capacity() {
            let a = self.push_back(k, spot);
            return Ok(a);
        }
        else {
            let p = self.open(spot, deltas);
            Ok(self.place(k, p))
        }
    }

    ///in-place reversal: pos 0 = max, names follow the slots (the
    ///translator absorbs it — no deltas; a pending event stays valid)
    fn reverse(&mut self) {
        self.data.reverse();
        self.mask.reverse();
        self.translator = self.translator.reverse(self.data.len());
        let len = self.len as u64;
        //reflect the bias signal around len — the trigger's own scale —
        // NOT the physical mirror (cap−1−p): cap imports density noise.
        // fires only when pos_sum < len²/8, so the subtraction never wraps
        self.pos_sum = len.wrapping_mul(len).wrapping_sub(self.pos_sum);
    }

    ///flip a front-biased insert stream so future inserts land at the
    ///tail. fires when mean insert position sits below len/8, committed
    ///past a 4·stride warmup. the check runs at insert's TOP — the trigger
    ///sees the pre-insert signal, and no translator mutation follows the
    ///event record inside insert (pos-space consumers rely on that)
    fn maybe_reverse(&mut self) {
        if self.len > 4 * FREE_STRIDE {
            let len = self.len as u64;
            // pos / len < len / 8
            if self.pos_sum.wrapping_mul(8) < len.wrapping_mul(len) {
                self.reverse();
            }
        }
    }

    ///insertion spot, or None on dup
    fn spot(&self, k: &K) -> Option<Pos> {
        if self.len > 0 {
            //beyond the tail extreme (max Asc, min Desc — the end a
            //sequential insert extends) the spot is known without bsearch
            let tail = self.key_tail();
            let beyond = if self.translator.reversed() { k < tail } else { tail < k };
            if beyond {
                return Some(Pos(self.mask.last_set().unwrap() + 1));
            }
        }
        self.bsearch(k).err()
    }

    ///vec-like first allocation: 4 slots, no translator turn until it fills
    fn insert_first(&mut self, k: K) -> A {
        self.data = (0..4).map(|_| MaybeUninit::uninit()).collect();
        self.mask = Mask::with_bits(4);
        self.place(k, Pos(0))
    }

    ///extend the allocation and place k at the edge — dense, holes never
    ///minted here. caller guarantees spot == cap < max_capacity
    fn push_back(&mut self, k: K, spot: Pos) -> A {
        self.push_slot();
        self.place(k, spot)
    }

    ///open a slot at/near `spot`: turns and gathers, escalating until a
    ///hole opens
    fn open(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Pos {
        let mut spot = spot;
        //preemptive turn past 7/8: at one hole per FREE_STRIDE expected,
        //fetches past 7/8 blow through the stride — one O(n) turn beats
        //the long gathers (mid-block inserts only)
        if self.len.wrapping_mul(TURN_DEN) > self.data.len().wrapping_mul(TURN_NUM)
            && self.data.len() - spot.0 > FREE_STRIDE
        {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            }
        }
        if let Some(p) = self.gather_slots(spot, deltas) {
            return p;
        }
        //mask full: one tail push when the edge is within FREE_STRIDE,
        //else a turn; refused ⇒ plain tail slot (room always exists:
        //mask full + len < name ceiling)
        if self.data.len() - spot.0 <= FREE_STRIDE && self.data.len() < Self::max_capacity() {
            self.push_slot();
        } else {
            let old = self.translator;
            if self.grow_rotate() {
                spot = self.turned_spot(old, spot);
            } else {
                self.push_slot();
            }
        }
        self.gather_slots(spot, deltas).expect("mask had a hole minted")
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
    fn place(&mut self, k: K, p: Pos) -> A {
        self.data[p.0] = MaybeUninit::new(k);
        self.mask.set(p.0, true);
        self.len += 1;
        self.pos_sum = self.pos_sum.wrapping_add(p.0 as u64);
        self.translator.p2a(p)
    }

    pub fn remove(&mut self, addr: A) -> Option<K> {
        self.remove_pos(self.translator.a2p(addr))
    }

    ///None if addr names no live element (stale-name hazard: get/remove_pos)
    fn remove_pos(&mut self, pos: Pos) -> Option<K> {
        if pos.0 < self.data.len() && self.mask.get(pos.0) {
            self.mask.set(pos.0, false);
            self.len -= 1;
            //mask ⇔ init invariant
            return Some(unsafe { self.data[pos.0].assume_init_read() });
        }
        None
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
        let big = self.data.len().wrapping_mul(std::mem::size_of::<K>()) > L2_BYTES;
        match (self.translator.reversed(), big) {
            (true, true) => self.search_spec::<true>(k),
            (true, false) => self.search_cmov::<true>(k),
            (false, true) => self.search_spec::<false>(k),
            (false, false) => self.search_cmov::<false>(k),
        }
    }

    ///speculating search — early exit, branch per probe. DRAM-resident
    ///blocks (see bsearch's split). the Equal early exit is LOAD-BEARING:
    ///gone, the two-way match compiles to cmov and the speculation (next
    ///probe's load issued ahead of the compare) dies — measured 1M lookup
    ///235→533
    fn search_spec<const REV: bool>(&self, k: &K) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (0, self.data.len());
        //holes skipped forward — everything live before the probe
        //compare known
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    //Desc: key order ascends against pos order — the
                    //compare operands flip (folded per instantiation)
                    let ord = if REV { k.cmp(probe) } else { probe.cmp(k) };
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
    fn search_cmov<const REV: bool>(&self, k: &K) -> Result<Pos, Pos> {
        let (mut lo, mut hi) = (0, self.data.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.next_live(mid, hi) {
                None => hi = mid,
                Some(p) => {
                    let probe = unsafe { self.key(p) };
                    let ord = if REV { k.cmp(probe) } else { probe.cmp(k) };
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
        if let Some(p) = self.next_live(lo, self.data.len()) {
            if unsafe { self.key(p) } == k {
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
    unsafe fn key(&self, p: usize) -> &K {
        unsafe { self.data[p].assume_init_ref() }
    }

    ///key at the physical tail (max Asc, min Desc); panics if len == 0
    fn key_tail(&self) -> &K {
        unsafe { self.key(self.mask.last_set().expect("live")) }
    }

    ///open a slot at/near `spot`; None ⇒ hole-free mask. the opened slot
    ///arrives as one `Delta` event (NoneSlide or Gather)
    fn gather_slots(&mut self, spot: Pos, deltas: &mut Deltas<A>) -> Option<Pos> {
        //k's slot already open (the bsearch converges on holes for
        //mid-gap inserts) — no fetch, no slide. nearest_clear excludes
        //`spot` by contract, so this is checked first
        if spot.0 < self.cap() && !self.mask.get(spot.0) {
            deltas.record_none_slide(self.translator, spot, spot);
            return Some(spot);
        }
        //the nearest hole's distance sets the fetch — one hole per
        //FREE_STRIDE traveled (the further the look, the more Nones come
        //back, banked beside k for the cluster's next inserts); holes all
        //from its side
        let hole = self.mask.nearest_clear(spot.0)?;
        //k's slot: hole right ⇒ `spot` (run shifts up), left ⇒ `spot − 1`
        //(run shifts down)
        let right = hole > spot.0;
        let open = if right { spot.0 } else { spot.0 - 1 };
        //the common case: hole within a stride — one plain slide, no fetch
        if hole.abs_diff(spot.0) < FREE_STRIDE {
            self.slide_hole(hole, open);
            deltas.record_none_slide(self.translator, Pos(hole), Pos(open));
            return Some(Pos(open));
        }
        let holes = self.fetch_holes(spot, right, 1 + hole.abs_diff(spot.0) / FREE_STRIDE);
        self.compact_gather(spot, open, right, &holes);
        deltas.record_gather(
            self.translator,
            holes
                .iter()
                .enumerate()
                .map(|(i, &h)| NoneSlide::new(Pos(h), Pos(if right { open + i } else { open - i })))
                .collect(),
        );
        Some(Pos(open))
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
        //update; the duplicate ptr::copy leaves in the source range is
        //uninit by mask, never assume_init'd
        self.mask.set(from, true);
        self.mask.set(to, false);
    }

    ///the `want` nearest holes on one side of `spot`, nearest-first
    ///(right: ascending, left: descending) — pre-read for
    ///`compact_gather`, no further scans
    fn fetch_holes(&self, spot: Pos, right: bool, want: usize) -> Vec<usize> {
        let mut holes = Vec::with_capacity(want);
        if right {
            let mut cursor = spot.0;
            while holes.len() < want
                && let Some(h) = self.mask.next_clear(cursor)
            {
                holes.push(h);
                cursor = h + 1;
            }
        } else {
            let mut cursor = spot.0 - 1;
            while holes.len() < want {
                let Some(h) = self.mask.prev_clear(cursor) else { break };
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
    ///member Some crossing exactly once. returns the slot k lands in
    ///(`open`)
    fn compact_gather(&mut self, spot: Pos, open: usize, right: bool, holes: &[usize]) {
        let w = holes.len();
        unsafe {
            let ptr = self.data.as_mut_ptr();
            //segment j (between consecutive holes) shifts by its crossing
            //count; far segments copy first so no live source is read
            //after being overwritten
            if right {
                //holes ascend; segment 0 spans [spot, h_0), the run k shifts
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
        let new_tr = self.translator.rotate();
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
        //the pos-space doubled — the insert-position sum tracks it
        self.pos_sum = self.pos_sum.wrapping_mul(2);
        true
    }

    ///extend by one uninit slot (mask clear) — the Vec's own doubling
    ///carries reallocation. panics at the name-space ceiling.
    fn push_slot(&mut self) {
        assert!(self.data.len() < Self::max_capacity(), "name space exhausted");
        self.data.push(MaybeUninit::uninit());
        self.mask.resize(self.data.len());
    }

    pub fn from_vec(v: Vec<K>) -> Self {
        let _ = v;
        todo!()
    }
}

///`BlockSet::iter`'s return type — direction picked at construction
pub enum Iter<'a, K, A> {
    Asc { bits: SetBits<'a>, set: &'a BlockSet<K, A> },
    Desc { bits: SetBitsRev<'a>, set: &'a BlockSet<K, A> },
}

impl<'a, K: Ord, A: UnsignedNum> Iterator for Iter<'a, K, A> {
    type Item = &'a K;

    #[inline]
    fn next(&mut self) -> Option<&'a K> {
        match self {
            //mask ⇔ init invariant upholds the read
            Iter::Asc { bits, set } => bits.next().map(|p| unsafe { set.key(p) }),
            Iter::Desc { bits, set } => bits.next().map(|p| unsafe { set.key(p) }),
        }
    }
}

///the slot `next` yields next, or the forward walk's exhaustion ghost.
///backward exhaustion needs no ghost: `prev` leaves the state at the
///first element, where `next` correctly re-yields it. pub(crate):
///blockmap's cursor is the same machine over pairs
pub(crate) enum CursorSlot {
    At(Pos),
    ///no next
    FwdEnd,
}

///key-ordered bidirectional walk — `seek`/`seek_after` position, `next`
///walks key-ascending (yield current, then step), `prev` walks back (step,
///then yield). steps run on a cached mask word (SetBits' optimization —
///dense steps test it in place; crossings reload). holds raw slots: any
///mutation (slides, turns, flips) invalidates it — re-seek
pub struct Cursor<'a, K, A> {
    set:  &'a BlockSet<K, A>,
    ///baked at construction — direction only changes by mutation, which
    ///invalidates anyway
    rev:  bool,
    ///the next-to-yield slot, or the forward ghost
    state: CursorSlot,
    ///state At(p) ⇒ p's mask word; FwdEnd leaves it stale (land reloads)
    word: u64,
}

impl<'a, K, A: UnsignedNum> Cursor<'a, K, A> {
    fn new(set: &'a BlockSet<K, A>) -> Self {
        Cursor {
            set,
            rev: set.translator.reversed(),
            state: CursorSlot::FwdEnd,
            word: 0,
        }
    }
}

impl<'a, K: Ord, A: UnsignedNum> Cursor<'a, K, A> {
    ///reposition at the first key ≥ k
    pub fn seek(&mut self, k: &K) {
        match self.set.bsearch(k) {
            Ok(p) => self.land(p.0),
            Err(spot) => match self.set.key_bound(spot.0) {
                Some(q) => self.land(q),
                None => self.state = CursorSlot::FwdEnd,
            },
        }
    }

    ///reposition at the first key > k (a dup at k skipped)
    pub fn seek_after(&mut self, k: &K) {
        let q = match self.set.bsearch(k) {
            //past the dup in key order
            Ok(p) => {
                if self.rev { self.set.mask.prev_set(p.0) } else { self.set.mask.next_set(p.0 + 1) }
            }
            Err(spot) => self.set.key_bound(spot.0),
        };
        if let Some(q) = q { self.land(q) } else { self.state = CursorSlot::FwdEnd }
    }

    ///position at a live slot — the cache load (seeks, ghost resumes)
    fn land(&mut self, q: usize) {
        self.word = self.set.mask.word_at(q);
        self.state = CursorSlot::At(Pos(q));
    }

    ///yield the current element, step forward (key-ascending). None at
    ///the walk's end.
    ///#[inline(always)]: plain #[inline] left next outlined (both
    ///directions' crossing scans inline into it, blowing LLVM's size
    ///budget) and walk loops paid a call + cursor spill per step —
    ///measured 2× @10k fwd
    #[inline(always)]
    pub fn next(&mut self) -> Option<&'a K> {
        let p = match self.state {
            CursorSlot::FwdEnd => return None,
            CursorSlot::At(p) => p.0,
        };
        //Asc climbs positions; Desc descends them
        let q = if self.rev {
            self.set.mask.step_down(p, self.word)
        } else {
            self.set.mask.step_up(p, self.word)
        };
        self.settle(q);
        Some(unsafe { self.set.key(p) })
    }

    ///step back (key-descending), yield the element now current. None at
    ///the walk's start (the state stays at the first element — next
    ///re-yields it); from the forward ghost, resumes at the last element
    #[inline(always)]
    pub fn prev(&mut self) -> Option<&'a K> {
        //the element behind the current — the walk's last from FwdEnd
        let q = match self.state {
            CursorSlot::At(p) => {
                if self.rev {
                    self.set.mask.step_up(p.0, self.word)
                } else {
                    self.set.mask.step_down(p.0, self.word)
                }
            }
            CursorSlot::FwdEnd => {
                let q = (if self.rev { self.set.mask.next_set(0) }
                    else { self.set.mask.last_set() })?;
                self.land(q);
                return Some(unsafe { self.set.key(q) });
            }
        }?;
        self.settle(Some(q));
        Some(unsafe { self.set.key(q.0) })
    }

    ///land a step result; None is the walk's end (the cache goes stale)
    fn settle(&mut self, q: Option<(usize, u64)>) {
        match q {
            Some((q, w)) => {
                self.word = w;
                self.state = CursorSlot::At(Pos(q));
            }
            None => self.state = CursorSlot::FwdEnd,
        }
    }

    ///the element next() would yield, without moving
    pub fn key(&self) -> Option<&'a K> {
        self.slot().map(|p| unsafe { self.set.key(p) })
    }

    ///the current element's name
    pub fn addr(&self) -> Option<A> {
        self.slot().map(|p| self.set.translator.p2a(Pos(p)))
    }

    ///At reads directly; FwdEnd has nothing ahead
    fn slot(&self) -> Option<usize> {
        match self.state {
            CursorSlot::At(p) => Some(p.0),
            CursorSlot::FwdEnd => None,
        }
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

    ///fetch-distance banking: holes 24 slots from the spot fetch 4 (one per
    ///FREE_STRIDE traveled) and land beside k — the next 3 inserts are
    ///no-ops. the 24 moved elements each emit ONE full-hop pair (crossing
    ///count 4; farthest-first, descending — the first-match order)
    #[test]
    fn gather_banks_holes_by_distance() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        for k in (0..=10u64).chain(60..=89).chain(200..=214) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        //holes at pos 35..=50 — the 35..40 half is 24 from the spot of
        //key 30 (the fetched side); the 41..50 half drops density below
        //the 3/4 turn line so the gather runs
        for k in (84..=89u64).chain(200..=209) {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        let a = set.insert(30, &mut d).unwrap();
        //identity translator (appends only): names = positions; the 4
        //consecutive fetched holes make segments 1..=3 empty — one
        //segment, hop 4, farthest element first
        assert_eq!(
            d.iter().collect::<Vec<_>>(),
            (11u8..=34).rev().map(|q| (q, q + 4)).collect::<Vec<_>>(),
            "24 moved elements, one hop-4 pair each"
        );
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 30));
        assert_eq!(set.cap(), 56, "gather must not turn");
        for k in [31, 32, 33] {
            let a = set.insert(k, &mut d).unwrap();
            assert_eq!(d.iter().count(), 0, "banked hole should absorb insert {k}");
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        check(&set, &holder, 44);
    }

    ///the LEFT-side gather mirror: holes below the spot, TWO non-empty
    ///segments — emission ascends (left walks up), far/low segment first,
    ///hop = crossing count per element
    #[test]
    fn gather_left_side_hop_pairs() {
        let mut set = BlockSet::<u64, u8>::new();
        let mut d = Deltas::new();
        let mut holder = Vec::new();
        for k in (0..=20u64).chain(100..=129) {
            let a = set.insert(k, &mut d).unwrap();
            apply(&mut holder, &d);
            d.clear();
            holder.push((a, k));
        }
        //holes at pos 0, 5, 10 (below the spot of key 50, nearest 11 away)
        //and 33..35, 38..40, 43, 48..50 above — the upper holes drop density
        //below the 3/4 turn line and sit farther than 11, so the gather
        //runs LEFT with two non-empty segments
        for k in [0u64, 5, 10, 112, 113, 114, 117, 118, 119, 122, 127, 128, 129] {
            set.remove(set.find(&k).unwrap());
            holder.retain(|(_, hk)| *hk != k);
        }
        let a = set.insert(50, &mut d).unwrap();
        //identity translator: segment [6, 10) hops −1 (far), [11, 21)
        //hops −2 — globally ascending `from`, every `to` behind
        assert_eq!(
            d.iter().collect::<Vec<_>>(),
            (6u8..=9)
                .chain(11..=20)
                .map(|q| (q, q - if q < 10 { 1 } else { 2 }))
                .collect::<Vec<_>>(),
            "14 moved elements, two non-empty segments, hop 1 then hop 2"
        );
        apply(&mut holder, &d);
        d.clear();
        holder.push((a, 50));
        check(&set, &holder, 39);
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


