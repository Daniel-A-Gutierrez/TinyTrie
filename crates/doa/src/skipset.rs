//!SkipSet trial: a deterministic skip-list index stacked over a `BlockSet`
//!base (the devlog sketch). tier0 WINDOW-FIRST SAMPLING: slot j holds a
//!COPIED handle (`K::Handle`, POD) of the FIRST live item in base window
//!j·STRIDE — or None when the whole window is empty. Nothing else can
//!empty a sample: gather banking, turn bands (odd-landing names) and
//!gridline holes all leave a first item in every occupied window. None
//!always means "no data in this window", so the unknown-slot descent
//!(the devlog's "IDK": empty slots don't narrow) only widens across
//!genuinely empty windows, and its range math is exact:
//!k < first(window w) ⇒ the answer lies strictly below w, so lo/hi are
//!the sample windows' gridlines — 32·last_le / 32·first_gt.
//!upper tiers sample every STRIDE-th slot of the tier below.
//!STORAGE: every tier lives in ONE allocation — slots + one mask over the
//!whole store (the substrate's own shape: bit ⇔ init), regions packed
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
use std::mem::MaybeUninit;

use crate::blockset::{BlockSet, Deltas, Iter};
use crate::index::UnsignedNum;
use crate::mask::Mask;
use crate::serialize::Archive;

///the settled skip stride — power of two (compile-time assert) so slot
///math is shift/mask; the div-vs-shift cost story lives in the header
pub(crate) const STRIDE: usize = 32;
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

///the single-allocation tier store: [top … tier0] regions + one mask
struct Store<K: Archive> {
    slots:   Vec<MaybeUninit<K::Handle>>,
    ///bit ⇔ slot initialized — the substrate's own invariant
    mask:    Mask,
    ///front-to-back, [top … tier0]
    regions: Vec<Region>,
}

impl<K: Archive> Store<K> {
    ///write tier i's slot j (mask ⇔ init — clear leaves stale bytes,
    ///never read)
    fn set(&mut self, tier: usize, slot: usize, handle: Option<K::Handle>) {
        let pos = self.regions[tier].start + slot;
        match handle {
            Some(handle) => {
                self.slots[pos] = MaybeUninit::new(handle);
                self.mask.set(pos, true);
            }
            None => self.mask.set(pos, false),
        }
    }

    ///tier i's slot j lifted from the tier below's grid slot j·STRIDE
    fn lifted(&self, tier: usize, slot: usize) -> Option<K::Handle> {
        let below = &self.regions[tier + 1];
        let grid_slot = slot * STRIDE;
        if grid_slot < below.width && self.mask.get(below.start + grid_slot) {
            // SAFETY: mask-gated
            Some(unsafe { *self.slots[below.start + grid_slot].assume_init_ref() })
        } else {
            None
        }
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

    pub fn iter(&self) -> Iter<'_, K, A> {
        self.base.iter()
    }

    pub fn get(&self, addr: A) -> Option<K::View<'_>> {
        self.base.get(addr)
    }

    ///deltas protocol unchanged — the base's events pass through. with
    ///tiers live: reversal is OURS (never the base's mid-insert flip),
    ///the tier descent bounds insert_at's search, and a ladder turn
    ///re-syncs via wholesale window re-read (maintain)
    pub fn insert(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        assert!(!deltas.is_pending(), "pending deltas: apply + clear before reuse");
        if self.store.is_none() {
            return self.base.insert(k, deltas);
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
            self.base.push_beyond(k, deltas)
        } else {
            let (blo, bhi) = self.window_of(&k);
            // SAFETY: the window contract holds — no live element before
            // `blo` in key order has key ≥ k, none at/after `bhi` has
            // key ≤ k; insert_at detects dups itself (Err before
            // archiving)
            unsafe { self.base.insert_at(k, deltas, blo, bhi) }
        };
        result.map(|(addr, placed)| {
            self.maintain(placed.0, deltas);
            addr
        })
    }

    ///bench phase-ablation hooks (see benches/skipset.rs): decompose the
    ///insert path by running prefixes of it and subtracting cells —
    ///timer granularity would swamp the phases. maintain-only also pays
    ///pos_of (the full path gets the Pos from insert_at) — a ~1.5ns
    ///overcount on the maintenance delta. NOT API
    #[doc(hidden)]
    pub fn insert_maintain_only(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        let result = self.base.insert(k, deltas);
        match &result {
            Ok(addr) => {
                if self.store.is_some() {
                    let placed = self.base.pos_of(*addr);
                    self.maintain(placed, deltas);
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
    pub fn insert_descent_only(&mut self, k: K, deltas: &mut Deltas<A>) -> Result<A, K> {
        if self.store.is_some() {
            let (blo, bhi) = self.window_of(&k);
            std::hint::black_box((blo, bhi));
        }
        self.insert_maintain_only(k, deltas)
    }

    ///post-insert tier upkeep: silent-move sync, grid extension, the
    ///slide chain — coalesced to ONE union interval (re-reads are
    ///idempotent), which always covers the placed item (the plain slide
    ///ends at `open`, the gather's first lands there, the no-op records
    ///(p, p)). push_back records nothing; push_beyond past tail holes
    ///records its slides like any insert — either way the union covers
    ///the placed item. the no-record path (a dense-tail push) updates
    ///the window's first iff nothing live precedes the placed item
    fn maintain(&mut self, placed: usize, deltas: &Deltas<A>) {
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
        for slide in deltas.slides() {
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
                store.set(store.regions.len() - 1, window, handle);
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
            slots: (0..total).map(|_| MaybeUninit::uninit()).collect(),
            mask: Mask::with_bits(total),
            regions,
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
        let mask = STRIDE_MASK;
        let Some(store) = self.store.as_mut() else { return };
        let tier_count = store.regions.len();
        let (mut lo, mut hi) = (from.min(to), from.max(to));
        for tier in (0..tier_count).rev() {
            let first_slot = (lo + mask) >> shift;
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
                store.set(tier, slot, handle);
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
            while scan < end {
                //next live slot in [scan, end) — mask-skipped holes
                let Some(live) = store.mask.next_set(scan) else { break };
                if live >= end {
                    break;
                }
                // SAFETY: mask-gated
                let handle = unsafe { *store.slots[live].assume_init_ref() };
                let slot = live - region.start;
                match compare(&handle) {
                    Ordering::Less => {
                        last_le = Some(slot);
                        scan = live + 1;
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
        let mut deltas = Deltas::new();
        let mut model = BTreeSet::new();
        let mut live = Vec::new();
        for (i, key) in keys.into_iter().enumerate() {
            //random streams carry dups — the model tracks only the accepted
            if set.insert(key, &mut deltas).is_err() {
                deltas.clear();
                continue;
            }
            deltas.clear();
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
            let mut deltas = Deltas::new();
            for &key in &keys[..100] {
                set.insert(key, &mut deltas).unwrap();
                deltas.clear();
            }
            set.rebuild();
            for &key in &keys[100..] {
                set.insert(key, &mut deltas).unwrap();
                deltas.clear();
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
        let mut deltas = Deltas::new();
        for key in (0..80u64).map(|i| i * 3 + 1) {
            set.insert(key, &mut deltas).unwrap();
            deltas.clear();
        }
        set.rebuild();
        let store = set.store.as_ref().unwrap();
        let tier_count = store.regions.len();
        for slot in 0..store.regions[tier_count - 1].width {
            let got = store.mask.get(store.regions[tier_count - 1].start + slot).then(|| unsafe {
                // SAFETY: mask-gated
                *store.slots[store.regions[tier_count - 1].start + slot].assume_init_ref()
            });
            assert_eq!(
                got,
                SkipSet::window_handle(&set.base, slot),
                "tier0 slot {slot}"
            );
        }
        for tier in 0..tier_count - 1 {
            for slot in 0..store.regions[tier].width {
                let want = store.lifted(tier, slot);
                let got = store.mask.get(store.regions[tier].start + slot).then(|| unsafe {
                    // SAFETY: mask-gated
                    *store.slots[store.regions[tier].start + slot].assume_init_ref()
                });
                assert_eq!(got, want, "tier {tier} slot {slot}");
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
        let mut deltas = Deltas::new();
        for key in (0..100u64).map(|i| i * 2) {
            set.insert(key, &mut deltas).unwrap();
            deltas.clear();
        }
        set.rebuild();
        for key in (100..200u64).map(|i| i * 2) {
            set.insert(key, &mut deltas).unwrap();
            deltas.clear();
        }
        assert!(set.contains(&198), "creep-maintained tiers");
        assert!(set.contains(&0));
        //mid insert into the dense block: turn fires → window re-read
        set.insert(45, &mut deltas).unwrap();
        deltas.clear();
        assert!(set.contains(&45), "after the turn");
        assert!(set.contains(&198));
        //post-turn holes absorb mid inserts — maintained, not rebuilt
        set.insert(91, &mut deltas).unwrap();
        deltas.clear();
        assert!(set.contains(&91));
        for key in (200..250u64).map(|i| i * 2) {
            set.insert(key, &mut deltas).unwrap();
            deltas.clear();
        }
        assert!(set.contains(&498));
        //descending bait flips the block — re-read, no rebuild
        for key in (500..560u64).rev() {
            set.insert(key, &mut deltas).unwrap();
            deltas.clear();
        }
        assert!(set.contains(&500) && set.contains(&559));
        let keys: Vec<u64> = set.iter().copied().collect();
        assert_matches_model(&set, keys.iter().copied(), 1);
        set.rebuild();
        assert_matches_model(&set, keys, 2);
    }
}