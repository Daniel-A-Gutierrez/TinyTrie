//!SkipSet trial: a deterministic skip-list index stacked over a `BlockSet`
//!base (the devlog sketch). tier0 WINDOW-FIRST SAMPLING: slot j holds a
//!COPIED handle (`K::Handle`, POD) of the FIRST live item in base window
//!j·STRIDE — an empty window BORROWS the left slot's handle (`K::MIN` at
//!the region head). real window-firsts strictly increase, so a REPEATED
//!value marks a hole: the descent treats repeats as transparent (the
//!devlog's "IDK": holes don't narrow, only widen — the safe direction),
//!and its range math is exact:
//!k < first(window w) ⇒ the answer lies strictly below w, so lo/hi are
//!the sample windows' gridlines — STRIDE·last_le / STRIDE·first_gt.
//!upper tiers lift the first REAL value of each STRIDE-wide below-grid
//!run (the block min); an all-empty run borrows in turn.
//!STORAGE: every tier lives in ONE allocation — handle slots only, NO
//!MASK (holes are borrowed values, detected by Eq), regions packed
//!front-to-back [top … tier0] with per-region reservations. tier0 is the
//!TAIL region: base-cap creep only extends its grid, so growth is
//!allocation-free within an epoch; regions are sized for the epoch's
//!ceiling (2× the sync-time cap doubled once more), so at most one turn
//!fits per epoch and creep never outgrows the reservation — ONE allocator
//!call per epoch instead of one per tier. STRIDE is a const power of two
//!(compile-time assert) so slot math is shift/mask — a runtime divisor
//!never compiles to a shift, and the div instructions once cost ~20× the
//!harmonic share of tier upkeep.
//!UPKEEP: incremental and silent-move-aware —
//!- slides/gathers: the deltas' compact slide chain names the changed
//!  position intervals; all slides of one insert COALESCE into their
//!  union interval [min, max] (re-reads are idempotent), applied as ONE
//!  pass, cascading upward as [ceil(lo/s), hi/s]. the placed item always
//!  lies inside the union. tail pushes record nothing — the placed item
//!  becomes its window's first iff nothing live precedes it there.
//!- turn (grow_rotate) / flip (manual reversal): the windows' CONTENTS
//!  moved, and only the block knows the new firsts — tier0 re-reads
//!  every window's first (never O(base)); upper tiers lift-fill.
//!- creep: widths track the base cap (extend); past the epoch reservation
//!  (turn refused / pathological growth) → full re-derive.
//!the top tier is rootless (skip-list shape, not a bounded-arity B-tree
//!node). removal unsupported (v0).

use std::cmp::Ordering;

use crate::blockset::{BlockSet, Deltas, Iter};
use crate::index::UnsignedNum;
use crate::serialize::Archive;

///the settled skip stride — power of two (compile-time assert) so slot
///math is shift/mask; the div-vs-shift cost story lives in the header
pub(crate) const STRIDE: usize = 16;
///log2(STRIDE)
const STRIDE_SHIFT: u32 = STRIDE.trailing_zeros();
///the grid mask, STRIDE − 1
const STRIDE_MASK: usize = STRIDE - 1;
const _: () = assert!(STRIDE >= 2 && STRIDE.is_power_of_two(), "STRIDE must be a power of 2 ≥ 2");

///one tier's footprint in the shared store: slots [start, start+width)
///used, [start+width, start+capacity) reserved — capacity is 2× the
///width chain at 2·cap, covering one turn doubling AND the grid
///extension to the epoch's cap ceiling
struct Region {
    start:    usize,
    width:    usize,
    capacity: usize,
}

///the single-allocation tier store: [top … tier0] regions, handle slots
///only — plain Vec (no MaybeUninit: set writes Some-or-borrow, so every
///in-width slot is initialized; reservation slots hold K::MIN and are
///never read)
struct Store<K: Archive> {
    slots:   Vec<K::Handle>,
    ///front-to-back, [top … tier0]
    regions: Vec<Region>,
    ///a borrow was ever written — set by set(None), reset by rebuild.
    ///false ⇒ the descent skips the repeat check entirely (dense store,
    ///the fast path); true ⇒ repeats mark holes and must be skipped
    holes: bool,
}

impl<K: Archive> Store<K> {
    ///write tier i's slot j. None = empty window/block: BORROW the left
    ///slot's handle (K::MIN at the region head) — a repeated value is the
    ///hole marker the descent detects. callers write ASCENDING within a
    ///region (fill, slide pass, sync, extend): the left slot is current
    ///when borrowed — load-bearing. FILL paths only: a full ascending
    ///fill leaves no stale holes; partial updates need set_cascade
    fn set(&mut self, tier: usize, slot: usize, handle: Option<K::Handle>) {
        let pos = self.regions[tier].start + slot;
        let handle = match handle {
            Some(handle) => handle,
            None => {
                self.holes = true;
                if slot == 0 {
                    K::MIN
                } else {
                    self.slots[pos - 1]
                }
            }
        };
        self.slots[pos] = handle;
    }

    ///maintenance write: set() plus the BORROW CASCADE — when the value
    ///at `slot` changes, the run of holes to its right that copied the
    ///OLD value are stale (a hole mirrors its left neighbor, and its left
    ///neighbor just changed); rewrite them with the new value. real
    ///values strictly increase, so `== old` names exactly that run
    fn set_cascade(&mut self, tier: usize, slot: usize, handle: Option<K::Handle>) {
        let (start, width) = {
            let region = &self.regions[tier];
            (region.start, region.width)
        };
        let pos = start + slot;
        let old = self.slots[pos];
        self.set(tier, slot, handle);
        let new = self.slots[pos];
        if new == old {
            return;
        }
        //rewrite the stale borrow run; stops at the first differing slot
        //(real or fresh) — never reads past region width (reservation)
        let mut i = pos + 1;
        let end = start + width;
        while i < end {
            let next = self.slots[i];
            if next != old {
                break;
            }
            self.slots[i] = new;
            i += 1;
        }
    }

    ///tier i's slot j lifted from the tier below: the first REAL value in
    ///the below-grid run [j·STRIDE, …) — real iff the value differs from
    ///its own left neighbor's (real values strictly increase; repeats are
    ///borrows). fast path: a real run head IS the block min (O(1)); a
    ///holed head scans forward. an all-empty run lifts None → the caller
    ///borrows here too
    fn lifted(&self, tier: usize, slot: usize) -> Option<K::Handle> {
        let below = &self.regions[tier + 1];
        let grid = slot * STRIDE;
        if grid >= below.width {
            return None;
        }
        //the run head's realness: differs from its left neighbor. a region
        //head (grid == 0) is real by convention — a MIN borrow there is
        //indistinguishable from a real K::MIN key; sound ONLY because the
        //descent seeds prev = K::MIN at lo == 0 and skips it (widening
        //both directions; a Desc head borrow comparing Greater once
        //narrowed bhi to 0 — every probe dead)
        let head = self.slots[below.start + grid];
        let real = grid == 0 || {
            let left = self.slots[below.start + grid - 1];
            head != left
        };
        if real {
            return Some(head);
        }
        let end = (grid + STRIDE).min(below.width);
        for i in grid + 1..end {
            if self.slots[below.start + i] != self.slots[below.start + i - 1] {
                return Some(self.slots[below.start + i]);
            }
        }
        None
    }
}

///maintenance-path counters — diagnostic tests only, zero release cost
#[cfg(test)]
#[derive(Default)]
struct Ops {
    ///escalations to a full rebuild (epoch reservation overflow)
    rebuilds:     usize,
    ///silent moves resolved by wholesale tier0 re-read (turns + flips —
    ///the only silent-move path under window-first sampling)
    rereads:      usize,
    ///creep grid extensions
    extends:      usize,
    ///coalesced slide-interval passes (incl. the tail-push slot set)
    slide_passes: usize,
    ///tail-push inserts routed to push_beyond
    tail_pushes:  usize,
}

///deterministic skip-list index over a `BlockSet` base: tiers built by
///`rebuild()`, maintained incrementally thereafter. removal unsupported
pub struct SkipSet<K: Archive, A: UnsignedNum> {
    base:     BlockSet<K, A>,
    ///None until the first rebuild
    store:    Option<Store<K>>,
    ///base pos-epoch at the last sync — mismatch = a silent position move
    epoch:    u64,
    #[cfg(test)]
    ops: Ops,
}

impl<K: Archive, A: UnsignedNum> SkipSet<K, A> {
    pub fn new() -> Self {
        SkipSet {
            base: BlockSet::new(),
            store: None,
            epoch: 0,
            #[cfg(test)]
            ops: Ops::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.base.len()
    }

    ///bench/diagnostic access to the base (census surface)
    #[doc(hidden)]
    pub fn base(&self) -> &BlockSet<K, A> {
        &self.base
    }

    pub fn iter(&self) -> Iter<'_, K, A> {
        self.base.iter()
    }

    pub fn get(&self, addr: A) -> Option<K::View<'_>> {
        self.base.get(addr)
    }

    ///the deltas protocol passes through the base's own slot — apply +
    ///exhaust before the next insert. with tiers live: reversal is OURS
    ///(never the base's mid-insert flip), the tier descent bounds
    ///insert_at's search, and a ladder turn re-syncs via wholesale window
    ///re-read (maintain)
    pub fn insert(&mut self, k: K) -> Result<A, K> {
        if self.store.is_none() {
            return self.base.insert(k);
        }
        if self.base.should_reverse() {
            self.base.reverse();
            self.sync_move();
        }
        //beyond the tail extreme: push_beyond — one tail scan total
        let result = if self.base.beyond_tail(&k) {
            #[cfg(test)]
            {
                self.ops.tail_pushes += 1;
            }
            self.base.push_beyond(k)
        } else {
            let (blo, bhi) = self.window_of(&k);
            // SAFETY: the window contract holds — no live element before
            // `blo` in key order has key ≥ k, none at/after `bhi` has
            // key ≤ k; insert_at detects dups itself (Err before
            // archiving)
            unsafe { self.base.insert_at(k, blo, bhi) }
        };
        result.map(|(addr, placed)| {
            self.maintain(placed.0);
            addr
        })
    }

    ///the base's event slot — the deltas protocol surface
    pub fn deltas(&mut self) -> &mut Deltas<A> {
        self.base.deltas()
    }

    ///bench phase-ablation hooks (see benches/skipset.rs): decompose the
    ///insert path by running prefixes of it and subtracting cells —
    ///timer granularity would swamp the phases. maintain-only also pays
    ///pos_of (the full path gets the Pos from insert_at) — a ~1.5ns
    ///overcount on the maintenance delta. NOT API
    #[doc(hidden)]
    pub fn insert_maintain_only(&mut self, k: K) -> Result<A, K> {
        let result = self.base.insert(k);
        match &result {
            Ok(addr) => {
                if self.store.is_some() {
                    let placed = self.base.pos_of(*addr);
                    self.maintain(placed);
                }
            }
            //a dup Err can still have auto-flipped the base mid-insert —
            //sync the epoch or every later find serves stale tiers
            Err(_) => {
                if self.store.is_some() && self.epoch != self.base.epoch() {
                    self.sync_move();
                }
            }
        }
        result
    }

    ///descent runs (and is discarded) before the base's own search
    #[doc(hidden)]
    pub fn insert_descent_only(&mut self, k: K) -> Result<A, K> {
        if self.store.is_some() {
            let (blo, bhi) = self.window_of(&k);
            std::hint::black_box((blo, bhi));
        }
        self.insert_maintain_only(k)
    }

    ///post-insert tier upkeep: silent-move sync, grid extension, the
    ///slide chain — coalesced to ONE union interval (re-reads are
    ///idempotent), which always covers the placed item (the plain slide
    ///ends at `open`, the gather's first lands there, the no-op records
    ///(p, p)). push_back records nothing; push_beyond past tail holes
    ///records its slides like any insert — either way the union covers
    ///the placed item. the no-record path (a dense-tail push) updates
    ///the window's first iff nothing live precedes the placed item
    fn maintain(&mut self, placed: usize) {
        //silent-move sync: the epoch moved since the last sync (a turn
        //inside the ladder, or the manual flip before the descent)
        if self.epoch != self.base.epoch() {
            self.sync_move();
        }
        //creep: the base cap outran tier0's grid (width·stride ≥ cap ⟺
        //width covers ceil(cap/stride))
        let store = self.store.as_ref().expect("maintain requires tiers");
        if self.base.cap() > store.regions.last().unwrap().width * STRIDE {
            self.extend();
        }
        let mut moved = false;
        let (mut lo, mut hi) = (usize::MAX, 0);
        for slide in self.base.deltas().slides() {
            lo = lo.min(slide.from.0.min(slide.to.0));
            hi = hi.max(slide.from.0.max(slide.to.0));
            moved = true;
        }
        if moved {
            self.apply_slide(lo, hi);
            #[cfg(test)]
            {
                self.ops.slide_passes += 1;
            }
        } else {
            //tail push: the placed item is its window's first iff no
            //live item precedes it in the window — the first-live read
            //picks it up exactly then (fresh windows, sparse tails)
            let window = placed >> STRIDE_SHIFT;
            if self.base.first_live_pos(window << STRIDE_SHIFT) == Some(placed) {
                let handle = self.base.slot(placed);
                let store = self.store.as_mut().unwrap();
                let tier0 = store.regions.len() - 1;
                store.set_cascade(tier0, window, handle);
                //empty→real at a GRIDLINE window changes the lifted value
                //of every upper tier whose grid names it — the old lift is
                //stale, but STALE-LIFTS ARE SAFE (a smaller-than-current
                //block min only widens; a borrow is still equal to its
                //left neighbor and repeat-detected). this re-lift is
                //TIGHTNESS, not correctness: it restores exact block mins
                //so later descents keep stride-width windows
                if window & STRIDE_MASK == 0 && tier0 > 0 {
                    let (mut tier, mut slot) = (tier0 - 1, window >> STRIDE_SHIFT);
                    loop {
                        let lifted = store.lifted(tier, slot);
                        store.set_cascade(tier, slot, lifted);
                        if slot & STRIDE_MASK != 0 || tier == 0 {
                            break;
                        }
                        slot >>= STRIDE_SHIFT;
                        tier -= 1;
                    }
                }
            }
        }
    }

    ///re-sync after a silent position move (turn in the ladder, or the
    ///manual flip) — the caller guarantees a moved epoch and built
    ///tiers. the windows' CONTENTS moved, and only the block knows the
    ///new firsts: tier0 re-reads every window's first (cap/STRIDE
    ///windows, never per-element); upper tiers lift-fill
    fn sync_move(&mut self) {
        let shift = STRIDE_SHIFT;
        let mask = STRIDE_MASK;
        let new_width = (self.base.cap() + mask) >> shift;
        let store = self.store.as_ref().expect("sync_move requires tiers");
        if new_width > store.regions.last().unwrap().capacity {
            //past the epoch's reservation (pathological growth) — full
            //re-derive
            #[cfg(test)]
            {
                self.ops.rebuilds += 1;
            }
            self.rebuild();
            return;
        }
        #[cfg(test)]
        {
            self.ops.rereads += 1;
        }
        let store = self.store.as_mut().unwrap();
        let tier_count = store.regions.len();
        store.regions[tier_count - 1].width = new_width;
        for window in 0..new_width {
            let handle = Self::window_handle(&self.base, window);
            store.set(tier_count - 1, window, handle);
        }
        //upper tiers follow the grid, lift-filled from tier0
        for tier in (0..tier_count - 1).rev() {
            let grown = (store.regions[tier + 1].width + mask) >> shift;
            store.regions[tier].width = grown;
            for slot in 0..grown {
                let handle = store.lifted(tier, slot);
                store.set(tier, slot, handle);
            }
        }
        self.epoch = self.base.epoch();
    }

    ///tier grid widths when the base cap is `cap` (tier0 first, top
    ///last) — the devlog ladder [root ≤ s][s²][s³]…: divide by stride
    ///per level, top ≤ stride. every descent scan then covers ≤ stride
    ///slots (the root scan and each child window) — a wider top (the
    ///s² misreading) measured +165ns/insert at 1M from the linear root
    ///scan alone
    fn chain(&self, cap: usize) -> Vec<usize> {
        let stride = STRIDE;
        let mask = stride - 1;
        let shift = STRIDE_SHIFT;
        let mut widths = vec![(cap + mask) >> shift];
        while *widths.last().unwrap() > stride {
            widths.push((widths.last().unwrap() + mask) >> shift);
        }
        widths
    }

    ///regions for a fresh epoch at base cap `cap`: current widths in
    ///CHAIN order (tier0 first, top last), reservations = 2× the chain
    ///at 2·cap — covers creep to the cap ceiling and one turn doubling
    fn fresh_regions(&self, cap: usize, mut widths: Vec<usize>) -> Vec<Region> {
        let ceil = self.chain(2 * cap);
        //the ceiling chain can carry one more tier than the current cap
        //does — pad at the top end (chain order's last), or the zip drops
        //the ceiling's top region and extend hits a missing region at the
        //epoch boundary. padded slots stay None until the below
        //grid grows into them
        while widths.len() < ceil.len() {
            let width = (widths.last().unwrap() + STRIDE_MASK) >> STRIDE_SHIFT;
            widths.push(width);
        }
        let mut regions: Vec<Region> = ceil
            .iter()
            .rev()
            .zip(widths.iter().rev())
            .map(|(&ceil_width, &width)| Region { start: 0, width, capacity: 2 * ceil_width })
            .collect();
        let mut start = 0;
        for region in regions.iter_mut() {
            region.start = start;
            start += region.capacity;
        }
        regions
    }

    ///full re-derivation — O(cap), handle copies only. the one allocator
    ///call of the epoch
    pub fn rebuild(&mut self) {
        let cap = self.base.cap();
        let widths = self.chain(cap);
        let regions = self.fresh_regions(cap, widths);
        let total: usize = regions.iter().map(|region| region.capacity).sum();
        let mut store = Store {
            slots: vec![K::MIN; total],
            regions,
            holes: false,
        };
        self.fill_all(&mut store);
        self.store = Some(store);
        self.epoch = self.base.epoch();
    }

    ///tier0 slot `window` ⇔ the FIRST live item in its window (None iff
    ///the window is empty) — the window-first contract: slots are Some
    ///whenever anything lives in the window, so banking, turn bands and
    ///gridline holes can never empty a sample. takes the base directly —
    ///callers hold a mutable store borrow (disjoint fields)
    fn window_handle(base: &BlockSet<K, A>, window: usize) -> Option<K::Handle> {
        let window_lo = window << STRIDE_SHIFT;
        match base.first_live_pos(window_lo) {
            Some(first) if first < window_lo + STRIDE => base.slot(first),
            _ => None,
        }
    }

    ///fill every tier: tier0 with window-firsts, the rest lifted from
    ///the tier below's grid slots
    fn fill_all(&self, store: &mut Store<K>) {
        let tier_count = store.regions.len();
        for window in 0..store.regions[tier_count - 1].width {
            let handle = Self::window_handle(&self.base, window);
            store.set(tier_count - 1, window, handle);
        }
        for tier in (0..tier_count - 1).rev() {
            for slot in 0..store.regions[tier].width {
                let handle = store.lifted(tier, slot);
                store.set(tier, slot, handle);
            }
        }
    }

    ///creep grid extension — the caller guarantees the base cap outran
    ///tier0's width and the tiers are built. allocation-free: the
    ///epoch's reservation covers the ceiling; past it, a full re-derive
    fn extend(&mut self) {
        let mask = STRIDE_MASK;
        let new_width = (self.base.cap() + mask) >> STRIDE_SHIFT;
        let store = self.store.as_ref().expect("extend requires tiers");
        if new_width > store.regions.last().unwrap().capacity {
            #[cfg(test)]
            {
                self.ops.rebuilds += 1;
            }
            self.rebuild();
            return;
        }
        {
            #[cfg(test)]
            {
                self.ops.extends += 1;
            }
            let store = self.store.as_mut().unwrap();
            let tier_count = store.regions.len();
            let old_width = store.regions[tier_count - 1].width;
            for window in old_width..new_width {
                let handle = Self::window_handle(&self.base, window);
                store.set(tier_count - 1, window, handle);
            }
            store.regions[tier_count - 1].width = new_width;
            for tier in (0..tier_count - 1).rev() {
                let grown = (store.regions[tier + 1].width + mask) >> STRIDE_SHIFT;
                let old = store.regions[tier].width;
                for slot in old..grown {
                    let handle = store.lifted(tier, slot);
                    store.set(tier, slot, handle);
                }
                store.regions[tier].width = grown;
            }
        }
    }

    ///re-read every window the changed-position interval [from, to]
    ///touched, cascading the interval upward — tier_{i}'s slot grid is
    ///every stride-th slot of the tier below, so the interval maps as
    ///[ceil(lo/s), hi/s] per level (shift/mask: stride a power of 2).
    ///level 0 re-reads window-firsts (a slide can change a window's
    ///first item even when it crosses no gridline); re-reads are
    ///idempotent
    fn apply_slide(&mut self, from: usize, to: usize) {
        let shift = STRIDE_SHIFT;
        let Some(store) = self.store.as_mut() else { return };
        let tier_count = store.regions.len();
        let (mut lo, mut hi) = (from.min(to), from.max(to));
        for tier in (0..tier_count).rev() {
            //FLOOR: position lo lives in window lo>>shift — the old
            //ceil skipped lo's own window whenever the interval started
            //mid-window (a subdivide's point event (mid, mid) then
            //concluded "nothing changed", leaving the window's first
            //stale-high — the descent narrowed the next insert's range
            //EXCLUDING its true boundary, and insert_at placed k into
            //a wrong-gap order break; repro: skipset runs@10k i=12)
            let first_slot = lo >> shift;
            let last_slot = hi >> shift;
            if first_slot > last_slot {
                break; //no slot on this tier's grid changed — none above
            }
            for slot in first_slot..=last_slot {
                let handle = if tier + 1 == tier_count {
                    Self::window_handle(&self.base, slot)
                } else {
                    store.lifted(tier, slot)
                };
                store.set_cascade(tier, slot, handle);
            }
            (lo, hi) = (first_slot, last_slot);
        }
    }

    ///tier descent → the base position window [blo, bhi) that must
    ///contain k's answer; find_window / insert_at resolve inside it.
    ///each level scans its slot RANGE for the last ≤ k and first > k
    ///entries; empty slots (None = empty WINDOW, under window-first
    ///sampling) don't narrow — the child range spans them. Desc bases:
    ///position order is key-reversed — the compare mirrors
    fn window_of(&self, k: &K) -> (usize, usize) {
        let Some(store) = &self.store else {
            unreachable!("window_of is only called with tiers built")
        };
        let rev = self.base.reversed();
        let compare = |handle: &K::Handle| {
            let ord = self.base.cmp_key(handle, k);
            if rev { ord.reverse() } else { ord }
        };
        let stride = STRIDE;
        let tier_count = store.regions.len();
        let (mut lo, mut hi) = (0usize, store.regions[0].width);
        for tier in 0..tier_count {
            let region = &store.regions[tier];
            let mut last_le = None;
            let mut first_gt = hi.min(region.width);
            let end = region.start + first_gt;
            let mut scan = region.start + lo;
            //hole detection: a borrowed slot repeats its left neighbor's
            //handle (real values strictly increase) — transparent to both
            //bounds. dense stores (never a borrow written) skip the check
            //outright. the HEAD seeds prev from slot lo−1: an inherited lo
            //can name a borrowed slot (a lifted parent value's grid origin
            //may itself be a hole), and a head hole repeats lo−1 exactly
            //— without the seed it poses as real and narrows last_le a
            //block too deep (the mask version's next_set skipped it). lo==0
            //seeds K::MIN instead: the region head holds a MIN borrow when
            //window 0 is empty, and under Desc that borrow compares Greater
            //(mirrored) and would narrow bhi to 0 — every probe dead.
            //skipping a head MIN is widening-only and safe in both
            //directions (a real head MIN — Asc, key MIN in window 0 —
            //drops to the leaf walk, which finds it)
            let holes = store.holes;
            let mut prev: Option<K::Handle> = match (holes, lo) {
                (false, _) => None,
                (true, 0) => Some(K::MIN),
                (true, _) => Some(store.slots[region.start + lo - 1]),
            };
            while scan < end {
                let handle = store.slots[scan];
                let slot = scan - region.start;
                scan += 1;
                if holes && prev == Some(handle) {
                    continue;
                }
                prev = Some(handle);
                match compare(&handle) {
                    Ordering::Less => {
                        last_le = Some(slot);
                    }
                    Ordering::Equal => {
                        last_le = Some(slot);
                        first_gt = slot + 1;
                        break;
                    }
                    Ordering::Greater => {
                        first_gt = slot;
                        break;
                    }
                }
            }
            if tier + 1 == tier_count {
                //tier0's range is base positions; bhi can reach
                //width·STRIDE one window past cap when cap isn't a
                //multiple — clamp to keep insert_at's `to ≤ cap` contract
                let blo = last_le.map_or(lo * stride, |slot| slot * stride);
                let bhi = (first_gt * stride).min(self.base.cap());
                return (blo, bhi);
            }
            lo = last_le.map_or(lo * stride, |slot| slot * stride);
            hi = first_gt * stride;
        }
        unreachable!("the tier loop always exits at the bottom region")
    }

    ///locate k: the tier descent, resolved against the base window
    pub fn find(&self, k: &K) -> Option<A> {
        if self.store.is_none() {
            return self.base.find(k);
        }
        let (blo, bhi) = self.window_of(k);
        self.base.find_window(k, blo, bhi)
    }

    pub fn contains(&self, k: &K) -> bool {
        self.find(k).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    ///insert stream, then verify find against a BTreeSet model over every
    ///key in the range. tiers built once mid-stream, then maintained
    ///incrementally through every slide/turn/flip the stream provokes
    fn assert_matches_model(set: &SkipSet<u64, u16>, keys: impl IntoIterator<Item = u64>, i: usize) {
        let model: BTreeSet<u64> = keys.into_iter().collect();
        assert_eq!(set.len(), model.len());
        let max = model.last().copied().unwrap_or(0) + 2;
        for key in 0..=max {
            assert_eq!(set.contains(&key), model.contains(&key), "i = {i} k = {key}");
            if let Some(addr) = set.find(&key) {
                assert_eq!(set.get(addr), Some(&key), "i = {i} k = {key}");
            }
        }
    }

    ///one rebuild mid-stream (i == 40), never again — the incremental
    ///maintenance must survive every subsequent slide, turn and flip
    fn torture(keys: impl IntoIterator<Item = u64>) {
        let mut set = SkipSet::<u64, u16>::new();
        let mut model = BTreeSet::new();
        let mut live = Vec::new();
        for (i, key) in keys.into_iter().enumerate() {
            //random streams carry dups — the model tracks only the accepted
            if set.insert(key).is_err() {
                set.deltas().clear();
                continue;
            }
            set.deltas().clear();
            model.insert(key);
            live.push(key);
            if i == 40 {
                set.rebuild();
            }
            assert_matches_model(&set, live.iter().copied(), i);
        }
        //a fresh rebuild must agree with the incrementally-maintained tiers
        assert_matches_model(&set, live.iter().copied(), 8888);
        set.rebuild();
        assert_matches_model(&set, live.iter().copied(), 9999);
    }

    ///one-off diagnostic: internal maintenance-path census over 10k-key
    ///insertion patterns — how often the epoch reservation escalates to
    ///a full rebuild vs works in place. the tiers are seeded by ONE
    ///explicit rebuild after 100 inserts; everything after is internal.
    ///run with --nocapture to see the table
    #[test]
    fn maintenance_paths_census() {
        let report = |name: &str, keys: Vec<u64>| {
            let mut set = SkipSet::<u64, u32>::new();
            for &key in &keys[..100] {
                set.insert(key).unwrap();
                set.deltas().clear();
            }
            set.rebuild();
            for &key in &keys[100..] {
                set.insert(key).unwrap();
                set.deltas().clear();
            }
            assert_eq!(set.len(), keys.len());
            let ops = &set.ops;
            println!(
                "{name:>8}: internal rebuilds {reb:>3} | in place: rereads {rr:>3} extends {ex:>3} slide_passes {sp:>5} | tail_pushes {tp:>5}",
                reb = ops.rebuilds,
                rr = ops.rereads,
                ex = ops.extends,
                sp = ops.slide_passes,
                tp = ops.tail_pushes,
            );
        };
        let n = 10_000u64;
        report("append", (0..n).collect());
        report("prepend", (0..n).rev().collect());
        report("random", {
            //xorshift-shuffled
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
            //100-key chunks, even chunks reversed, chunk order shuffled
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

    #[test]
    fn matches_model_ascending() {
        torture(0..300u64);
    }

    ///descending stream flips the base — tiers re-read and the compare
    ///mirrors
    #[test]
    fn matches_model_descending() {
        torture((0..300u64).rev());
    }

    #[test]
    fn matches_model_stride() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        torture((0..300u64).map(|_| rnd() % 500));
    }

    ///tier structure: tier0 slot j = the handle of the FIRST live item
    ///in window j·STRIDE (None iff the window is empty); each upper tier
    ///lifts every stride-th grid slot of the tier below; regions hold
    ///the epoch reservation
    #[test]
    fn tier_sampling() {
        let mut set = SkipSet::<u64, u16>::new();
        for key in (0..80u64).map(|i| i * 3 + 1) {
            set.insert(key).unwrap();
            set.deltas().clear();
        }
        set.rebuild();
        let store = set.store.as_ref().unwrap();
        let tier_count = store.regions.len();
        //tier0: the window's first, holes borrowed left (K::MIN at the head)
        let mut prev = u64::MIN;
        for slot in 0..store.regions[tier_count - 1].width {
            let want = SkipSet::window_handle(&set.base, slot).unwrap_or(prev);
            let got = store.slots[store.regions[tier_count - 1].start + slot];
            assert_eq!(got, want, "tier0 slot {slot}");
            prev = want;
        }
        //upper tiers: lifted block-mins, all-empty runs borrowed left
        for tier in 0..tier_count - 1 {
            let mut prev = u64::MIN;
            for slot in 0..store.regions[tier].width {
                let want = store.lifted(tier, slot).unwrap_or(prev);
                let got = store.slots[store.regions[tier].start + slot];
                assert_eq!(got, want, "tier {tier} slot {slot}");
                prev = want;
            }
        }
        //regions: [top … tier0], each with its epoch reservation
        for region in &store.regions {
            assert!(region.width <= region.capacity, "reservation");
        }
    }

    ///tiers built once, then: appends (creep — window extension, no
    ///allocation), a turn (wholesale window re-read), further mid
    ///inserts, and a descending tail (manual flip) — finds must hold
    ///without any explicit rebuild
    #[test]
    fn incremental_maintenance() {
        let mut set = SkipSet::<u64, u16>::new();
        for key in (0..100u64).map(|i| i * 2) {
            set.insert(key).unwrap();
            set.deltas().clear();
        }
        set.rebuild();
        for key in (100..200u64).map(|i| i * 2) {
            set.insert(key).unwrap();
            set.deltas().clear();
        }
        assert!(set.contains(&198), "creep-maintained tiers");
        assert!(set.contains(&0));
        //mid insert into the dense block: turn fires → window re-read
        set.insert(45).unwrap();
        set.deltas().clear();
        assert!(set.contains(&45), "after the turn");
        assert!(set.contains(&198));
        //post-turn holes absorb mid inserts — maintained, not rebuilt
        set.insert(91).unwrap();
        set.deltas().clear();
        assert!(set.contains(&91));
        for key in (200..250u64).map(|i| i * 2) {
            set.insert(key).unwrap();
            set.deltas().clear();
        }
        assert!(set.contains(&498));
        //descending bait flips the block — re-read, no rebuild
        for key in (500..560u64).rev() {
            set.insert(key).unwrap();
            set.deltas().clear();
        }
        assert!(set.contains(&500) && set.contains(&559));
        let keys: Vec<u64> = set.iter().copied().collect();
        assert_matches_model(&set, keys.iter().copied(), 1);
        set.rebuild();
        assert_matches_model(&set, keys, 2);
    }

    ///REGRESSION (apply_slide's mid-window miss): the bench's runs shape
    ///at 10k, maintained phase — a subdivide's point event (mid, mid)
    ///starting mid-window left tier0's window-first stale-high; the
    ///descent narrowed the next insert's range EXCLUDING its true
    ///boundary, insert_at placed k into a wrong gap, and order broke at
    ///i=12. full order check after every insert, both phases
    #[test]
    fn maintained_runs_order() {
        let n = 10000usize;
        let mut chunks: Vec<Vec<u64>> = (0..n / 100)
            .map(|c| ((c * 100) as u64..((c * 100) as u64 + 100)).collect())
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
        let keys: Vec<u64> = chunks.into_iter().flatten().collect();
        let mut set = SkipSet::<u64, u32>::new();
        let half = keys.len() / 2;
        for &k in &keys[..half] {
            set.insert(k).unwrap_or_else(|k| panic!("dup {k}"));
            set.deltas().clear();
        }
        set.rebuild();
        for (i, &k) in keys[half..].iter().enumerate() {
            set.insert(k).unwrap_or_else(|k| panic!("dup {k}"));
            set.deltas().clear();
            let mut prev = None;
            for x in set.iter().copied() {
                if let Some(p) = prev {
                    assert!(x > p, "maint i={i} k={k}: order {p} !< {x}");
                }
                prev = Some(x);
            }
            assert_eq!(set.len(), half + i + 1);
        }
    }
}