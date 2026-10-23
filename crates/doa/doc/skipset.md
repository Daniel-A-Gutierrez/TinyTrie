```rust
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
///L0047
///the settled skip stride — power of two (compile-time assert) so slot
///math is shift/mask; the div-vs-shift cost story lives in the header
pub(crate) const STRIDE: usize = 16;
///L0049
///log2(STRIDE)
const STRIDE_SHIFT: u32 = STRIDE.trailing_zeros();
///L0051
///the grid mask, STRIDE − 1
const STRIDE_MASK: usize = STRIDE - 1;
///L0052
const _: () = assert!(STRIDE >= 2 && STRIDE.is_power_of_two(), "STRIDE must be a power of 2 ≥ 2");
///L0058
///one tier's footprint in the shared store: slots [start, start+width)
///used, [start+width, start+capacity) reserved — capacity is 2× the
///width chain at 2·cap, covering one turn doubling AND the grid
///extension to the epoch's cap ceiling
struct Region {
    start:    usize,
    width:    usize,
    capacity: usize,
}
///L0068
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
///L0078
impl<K: Archive> Store<K> {}
///L0171
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
///L0187
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
///L0197
impl<K: Archive, A: UnsignedNum> SkipSet<K, A> {}
///L0674
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    ///insert stream, then verify find against a BTreeSet model over every
    ///key in the range. tiers built once mid-stream, then maintained
    ///incrementally through every slide/turn/flip the stream provokes
    fn assert_matches_model(set: &SkipSet<u64, u16>, keys: impl IntoIterator<Item = u64>, i: usize);
    ///one rebuild mid-stream (i == 40), never again — the incremental
    ///maintenance must survive every subsequent slide, turn and flip
    fn torture(keys: impl IntoIterator<Item = u64>);
    ///one-off diagnostic: internal maintenance-path census over 10k-key
    ///insertion patterns — how often the epoch reservation escalates to
    ///a full rebuild vs works in place. the tiers are seeded by ONE
    ///explicit rebuild after 100 inserts; everything after is internal.
    ///run with --nocapture to see the table
    #[test]
    fn maintenance_paths_census();
    #[test]
    fn matches_model_ascending();
    ///descending stream flips the base — tiers re-read and the compare
    ///mirrors
    #[test]
    fn matches_model_descending();
    #[test]
    fn matches_model_stride();
    ///tier structure: tier0 slot j = the handle of the FIRST live item
    ///in window j·STRIDE (None iff the window is empty); each upper tier
    ///lifts every stride-th grid slot of the tier below; regions hold
    ///the epoch reservation
    #[test]
    fn tier_sampling();
    ///tiers built once, then: appends (creep — window extension, no
    ///allocation), a turn (wholesale window re-read), further mid
    ///inserts, and a descending tail (manual flip) — finds must hold
    ///without any explicit rebuild
    #[test]
    fn incremental_maintenance();
    ///REGRESSION (apply_slide's mid-window miss): the bench's runs shape
    ///at 10k, maintained phase — a subdivide's point event (mid, mid)
    ///starting mid-window left tier0's window-first stale-high; the
    ///descent narrowed the next insert's range EXCLUDING its true
    ///boundary, insert_at placed k into a wrong gap, and order broke at
    ///i=12. full order check after every insert, both phases
    #[test]
    fn maintained_runs_order();
}
```
