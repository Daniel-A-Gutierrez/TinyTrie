//! Strategy module — owns block *mutation*: how an insert is placed (into an
//! on-AP `None` / append with gap-insertion), how a removal shifts the
//! resulting `None` to an aligned slot (or no-ops), and the per-strategy
//! `budget` (stride-steps `find_slot` scans per direction before giving up).
//! Produces [`InsertDelta`] for callers to remap pointers.
//!
//! `Block` stays thin (`buf` + `addr_shift`/`none_mask`/`virt_offset` + find);
//! the strategy owns the mutation policy. A [`BlockStrategy`] tag on `Block`
//! carries strategy state so dispatch doesn't infer it from `(addr_shift,
//! none_mask)` (ambiguous: append & prepend both have `addr_shift = 0`).
//!
//! # AP-maintenance invariant
//!
//! `find_slot` walks the eligible AP (aligned positions, stride =
//! `none_mask + 1`); to stay fast the AP must stay stocked with `None`
//! vacancies. Rules (user-confirmed):
//!
//! 1. **`push_back/push_front`** (append/prepend) inserts `None` gaps — one every
//!    `none_stride` (≈16) — to stock the AP (gap-insertion; append done,
//!    prepend TODO).
//! 2. **Mid-block insert**: reuse an on-AP `None` within budget (`Found::At`
//!    → `Free`); else `NotFound`. The block layer does NOT shift-to-create a
//!    `None` on insert — that's the arena's job (Step 4).
//! 3. **Mid-block `remove`**: shift the resulting `None` to the nearest
//!    aligned slot IF cheap-and-easy (≤ stride/2) and an in-range `Some`
//!    aligned slot exists to absorb it — UNLESS both nearest aligned spots
//!    are already `None` (no-op; the unaligned `None` is bracketed). Returns
//!    `InsertDelta::Move` (or `Free` if no shift).
//! 4. **Invariant**: unaligned `None`s are tolerated only between aligned
//!    `None`s (both flanking AP positions `None`); outside that bracket,
//!    unaligned slots must be `Some`. Net: keep aligned positions `None` when
//!    possible; tolerate unaligned `None`s only when bracketed. Less wasteful
//!    than "always shift to align".
//!
//! # Status
//!
//! Step 3b landed: gap-insertion on append + removal shift-to-AP. NOT done:
//! prepend gap-insertion, graduation (`post_insert_check` is a no-op stub),
//! spread/split (Step 4). The block layer has no insert-failure seam —
//! `NotFound` propagates to the caller (arena splits, Step 4).
//!
//! # Translation-field access
//!
//! `Block`'s `buf`/`addr_shift`/`none_mask`/`virt_offset` are `pub(crate)` so
//! strategy methods (which take `&mut Block`) can touch them directly — same
//! `(p + v_off_phys) & none_mask == 1` AP-eligibility test as `find_slot`.

use std::marker::PhantomData;

use crate::block::Block;
use crate::find_slot::{Found, NotFound};
use crate::index::SignedBlockIndex;

/// Stride-steps scanned per direction before giving up. Append/prepend
/// strategies use this as their budget; pluripotent/random use `cap` (the
/// whole block). Kept here (not on `Block`) so the strategy is the single
/// source — `find_slot` reads it via `self.strategy.budget()`.
pub(crate) const INSERT_BUDGET: usize = 8;

/// `cap` must be a power of two `>= 1` — `addr_shift` is computed via
/// `trailing_zeros()` subtraction, so a non-power-of-two or zero `cap`
/// silently corrupts the geometry in release. Hard `assert!` (math
/// correctness, not a config catch).
pub(crate) fn assert_cap_pow2(cap: usize) {
    assert!(cap.is_power_of_two(), "cap must be a power of two >= 1, got {cap}",);
}

// -----------------------------------------------------------------------
// InsertDelta — remap info produced by insert/remove for pointer fixup.
// -----------------------------------------------------------------------

/// Outcome of an insert or removal — the info callers need to remap pointers
/// into the block after a mutation.
///
/// Generic over `T` for [`BlockSplit`] (Step 4 carries moved-out values / new
/// block ids); the Step 2 variants (`Free`/`Move`) hold no `T`. `Move` serves
/// both insert-shift and removal-shift (same element-movement shape). Step 2
/// only produces `Free`; `Move`/`BlockSplit` are fixed here for Step 3/4.
#[derive(Debug)]
pub enum InsertDelta<T> {
    /// Placed into a pre-existing `None` slot (or appended/prepended) — no
    /// other elements moved. `new_virt` is the new element's virtual address
    /// (insert) or the freed slot's address (removal no-op). Address-stable: no
    /// pointer remapping needed.
    Free { new_virt: isize },

    /// Elements were shifted to create room (insert) or re-align vacancies
    /// (remove). 
    /// `new_virt` = the new element's address (insert) or the shifted
    /// region's anchor (removal). 
    /// `amount` = positions each element moved;
    /// `minus` = per-element address-delta sign (`+1`/`-1`); 
    /// `addr_delta` = `minus << addr_shift` (the remap callers apply).
    ///
    /// TODO Step 3: graduation rewrites `addr_shift` + `virt_offset`, so each
    /// element's address changes by a PER-PHYS function, not a uniform
    /// `addr_delta` over a range — `Move` can't describe it. Graduation likely
    /// needs its own variant (e.g. `Graduate { old_shift, new_shift,
    /// old_offset, new_offset }`). Deferred to Step 3.
    Move { new_virt: isize, amount: usize, minus: isize, addr_delta: isize },

    /// Block split placeholder — Step 4 fills the spread/split producing a new
    /// block and cross-block remap info. `PhantomData<T>` marks the generic;
    /// Step 4 will carry moved-out values / new block ids.
    ///
    /// TODO Step 4: splits allocate a block id + link prev/next (arena-level;
    /// `Arena` owns `VecDeque<Block>`), but strategy methods only get `&mut
    /// Block`. `BlockSplit` may belong to the arena, not the strategy —
    /// revisit the shape in Step 4.
    BlockSplit { _phantom: PhantomData<T> },
}

impl<T> InsertDelta<T> {
    /// The new element's virtual address, if any. `Free { new_virt }` →
    /// `Some(new_virt)`; `Move { new_virt, .. }` → `Some(new_virt)`; `BlockSplit`
    /// → `None` (Step 4 will define its own extraction).
    pub fn new_virt(&self) -> Option<isize> {
        match self {
            InsertDelta::Free { new_virt } => Some(*new_virt),
            InsertDelta::Move { new_virt, .. } => Some(*new_virt),
            InsertDelta::BlockSplit { .. } => None,
        }
    }
}

// -----------------------------------------------------------------------
// BlockStrategy — tag enum stored on Block for dispatch.
// -----------------------------------------------------------------------

/// Tag identifying which growth strategy a block uses. Stored as a field on
/// [`Block`] so dispatch doesn't infer strategy from `(addr_shift, none_mask)`
/// (ambiguous: append & prepend both have `addr_shift == 0`).
pub(crate) enum BlockStrategy {
    /// Balanced growth. Budget = `cap` (the whole block —
    /// per notes, "budget is the entire block so it can't fail"). Graduates to
    /// a concrete strategy at `len == half_ptr` (Step 3 `post_insert_check`).
    Pluripotent(PluripotentStrategy),
    /// End-optimized for `push_back`. Budget = `INSERT_BUDGET` (small —
    /// end-optimized blocks quickly reach the viable back end).
    Append(AppendStrategy),
    /// End-optimized for `push_front`. Budget = `INSERT_BUDGET` (small).
    Prepend(PrependStrategy),
    /// Full-range addressing. Budget = `cap` (conservative default — a generous
    /// budget avoids premature `OutOfBudget` on sparse blocks).
    Random(RandomStrategy),
}

/// `Default` is the `mem::take` placeholder — a `Random` with `INSERT_BUDGET`.
/// It's immediately replaced by the real strategy after the dispatch call, so
/// it's never observed by user code. `INSERT_BUDGET` (not 0) so
/// `from_raw_parts`/`new` keep the same scan behavior as before the migration.
///
/// NOTE: a DIFFERENT budget than `RandomStrategy::new_block` (which sets
/// `budget = cap`). `from_raw_parts`/`new` blocks scan with `INSERT_BUDGET`=8;
/// `new_random` blocks scan with `cap`. The divergence is intentional
/// (raw-parts blocks are bench/test scaffolding, not the strategy-initialized
/// path) — don't assume `Default` == `new_random`.
impl Default for BlockStrategy {
    fn default() -> Self {
        BlockStrategy::Random(RandomStrategy { budget: INSERT_BUDGET })
    }
}

impl BlockStrategy {
    /// Per-strategy scan budget: stride-steps scanned per direction by
    /// `find_slot` before giving up. Pluripotent/random use `cap` (whole
    /// block); append/prepend use `INSERT_BUDGET` (small). The single source
    /// for `find_slot` (`self.strategy.budget()` — a field read + enum match,
    /// inlined/const-foldable). Not on the `GrowthStrategy` trait — budget is
    /// config, not mutation behavior.
    #[inline]
    pub(crate) fn budget(&self) -> usize {
        match self {
            BlockStrategy::Pluripotent(s) => s.budget,
            BlockStrategy::Append(s) => s.budget,
            BlockStrategy::Prepend(s) => s.budget,
            BlockStrategy::Random(s) => s.budget,
        }
    }

    /// Set the scan budget on the current variant. Used by `Block::with_budget`
    /// (test/bench) and by the graduation seam (Step 3: pluripotent→concrete
    /// changes budget from `cap` to a small value).
    #[inline]
    pub(crate) fn set_budget(&mut self, budget: usize) {
        match self {
            BlockStrategy::Pluripotent(s) => s.budget = budget,
            BlockStrategy::Append(s) => s.budget = budget,
            BlockStrategy::Prepend(s) => s.budget = budget,
            BlockStrategy::Random(s) => s.budget = budget,
        }
    }

    /// Dispatch insert handling to the inner strategy struct. See
    /// [`GrowthStrategy::handle_insertion`].
    pub(crate) fn handle_insertion<T, PTR: SignedBlockIndex, const OP: bool>(
        &mut self,
        block: &mut Block<T, PTR, OP>,
        found: Found,
        hint_phys: usize,
        value: T)
        -> Result<InsertDelta<T>, NotFound> {
        match self {
            BlockStrategy::Pluripotent(s) => s.handle_insertion(block, found, hint_phys, value),
            BlockStrategy::Append(s) => s.handle_insertion(block, found, hint_phys, value),
            BlockStrategy::Prepend(s) => s.handle_insertion(block, found, hint_phys, value),
            BlockStrategy::Random(s) => s.handle_insertion(block, found, hint_phys, value),
        }
    }

    /// Dispatch removal handling to the inner strategy struct. See
    /// [`GrowthStrategy::handle_removal`].
    pub(crate) fn handle_removal<T, PTR: SignedBlockIndex, const OP: bool>(&mut self,
                                                                           block: &mut Block<T, PTR, OP>,
                                                                           phys: usize)
                                                                           -> InsertDelta<T> {
        match self {
            BlockStrategy::Pluripotent(s) => s.handle_removal(block, phys),
            BlockStrategy::Append(s) => s.handle_removal(block, phys),
            BlockStrategy::Prepend(s) => s.handle_removal(block, phys),
            BlockStrategy::Random(s) => s.handle_removal(block, phys),
        }
    }

    /// Graduation seam dispatch. See [`GrowthStrategy::post_insert_check`].
    pub(crate) fn post_insert_check<T, PTR: SignedBlockIndex, const OP: bool>(
        &mut self,
        block: &mut Block<T, PTR, OP>)
        -> Option<InsertDelta<T>> {
        match self {
            BlockStrategy::Pluripotent(s) => s.post_insert_check(block),
            BlockStrategy::Append(s) => s.post_insert_check(block),
            BlockStrategy::Prepend(s) => s.post_insert_check(block),
            BlockStrategy::Random(s) => s.post_insert_check(block),
        }
    }
}

// -----------------------------------------------------------------------
// GrowthStrategy trait
// -----------------------------------------------------------------------

/// The mutation contract: how a block handles inserts, removals, and
/// graduation. Each strategy struct impls this; [`BlockStrategy`] delegates
/// via `Block::try_insert_at`/`remove` (which `mem::take` the strategy out, call
/// the trait method with `&mut Block`, then put it back — the borrow-checker
/// clean way to give the strategy `&mut self` + `&mut Block` simultaneously).
///
/// `handle_insertion` handles `Found::At` (place into the on-AP `None`, `Free`),
/// `Found::Append`/`Prepend` (push; append path gap-inserts to stock the AP).
/// The block layer does NOT shift-to-create on insert — `NotFound` propagates
/// to the caller (arena splits, Step 4). `handle_removal` (default impl) shifts
/// the resulting `None` to the nearest aligned slot (or no-op) — see its doc.
/// `post_insert_check` is a no-op graduation stub (Step 3).
///
/// See the [module-level docs](self) for the AP-maintenance invariant.
pub(crate) trait GrowthStrategy<T, PTR: SignedBlockIndex, const OP: bool> {
    /// Handle a successful `find_slot` result: place the value. `hint_phys` is
    /// the anchor the caller wanted to insert near (unused at the block layer —
    /// the directional bias is in `find_slot` itself; no insert-shift here).
    /// Consumes `value` (placed on Ok, dropped on Err). Returns an
    /// [`InsertDelta<T>`] so callers can remap pointers.
    ///
    /// `Found::At` → place into the on-AP `None` (`Free`). `Found::Append` →
    /// `push_back` (with gap-insertion when `gap_on_append`). `Found::Prepend`
    /// → `push_front` (prepend gap-insertion TODO). All return `Free` (no
    /// element moves on the insert path). Spread/split is Step 4 (arena).
    fn handle_insertion(&mut self,
                        block: &mut Block<T, PTR, OP>,
                        found: Found,
                        hint_phys: usize,
                        value: T)
                        -> Result<InsertDelta<T>, NotFound>;

    /// Handle a removal: after the element at `phys` is set to `None` by
    /// `Block::remove`, shift elements so the resulting vacancy lands on an
    /// aligned (AP) spot — IF cheap-and-easy and an in-range `Some` aligned slot
    /// exists — unless both nearest aligned spots are already `None` (no-op; the
    /// unaligned `None` is bracketed). Returns [`InsertDelta::Move`] with the
    /// remap info, or [`InsertDelta::Free`] if no shift was needed (already
    /// aligned, no AP, or no `Some` aligned target). This is the shared default
    /// — all strategies use it.
    fn handle_removal(&mut self, block: &mut Block<T, PTR, OP>, phys: usize) -> InsertDelta<T> {
        // Step 3b (removal shift-to-AP): after `Block::remove` `None`'d `phys`,
        // shift elements so the `None` moves to the nearest aligned (AP) slot
        // — IF cheap-and-easy (always short: the nearest aligned slot is ≤
        // stride-1 away) and an in-range `Some` aligned slot exists to absorb
        // the shift. No-op when `phys` is already aligned, or when no in-range
        // flanking aligned slot is `Some` (both `None` → the unaligned `None`
        // is bracketed by aligned vacancies and tolerated; out-of-range → no
        // target). User 2026-07-11: "shift the none we create by removing to a
        // usable space for further inserts if its cheap and easy."
        let none_mask = block.none_mask as usize;
        let v_off_phys = (block.virt_offset as usize) >> block.addr_shift;
        let stride = none_mask + 1;
        let len = block.buf.len();

        // No AP (stride 1, dense block) → "shift to AP" is meaningless; no-op.
        if none_mask == 0 {
            return InsertDelta::Free { new_virt: block.phys_to_virt(phys) };
        }

        // `phys` already on the AP → no shift.
        if phys.wrapping_add(v_off_phys) & none_mask == 1 {
            return InsertDelta::Free { new_virt: block.phys_to_virt(phys) };
        }

        // Nearest flanking aligned slots. `right_delta` = distance to the
        // aligned slot at/after `phys` (1..=stride-1 since `phys` is unaligned).
        let residue = phys.wrapping_add(v_off_phys) & none_mask;
        let right_delta = (1usize.wrapping_sub(residue)) & none_mask;
        let first_right = phys as isize + right_delta as isize; // > phys
        let first_left = phys as isize - (stride - right_delta) as isize; // < phys

        let right_in = first_right < len as isize;
        let left_in = first_left >= 0;
        let right_none = right_in && block.buf[first_right as usize].is_none();
        let left_none = left_in && block.buf[first_left as usize].is_none();
        // Only an in-range `Some` aligned slot is a valid shift target.
        let right_target = right_in && !right_none;
        let left_target = left_in && !left_none;
        if !right_target && !left_target {
            // No `Some` aligned slot to absorb the shift (both flanking `None`
            // or out-of-range) → tolerate the unaligned `None`. No-op.
            return InsertDelta::Free { new_virt: block.phys_to_virt(phys) };
        }

        // Pick the nearer target (cheapest shift). Tie → right.
        let right_dist = right_delta;
        let left_dist = stride - right_delta;
        let go_right = right_target && (!left_target || right_dist <= left_dist);

        if go_right {
            // Move the `None` RIGHT to `first_right`: elements phys+1..A shift
            // left by 1 (each phys -= 1 → virt -= 1<<addr_shift). The `None`
            // propagates phys → A via adjacent swaps.
            let a = first_right as usize;
            for i in (phys + 1)..=a {
                block.buf.swap(i, i - 1);
            }
            let minus = -1isize;
            let addr_delta = minus.checked_shl(block.addr_shift)
                                  .expect("handle_removal: addr_shift >= isize::BITS");
            InsertDelta::Move { new_virt: block.phys_to_virt(a),
                                amount: right_dist,
                                minus,
                                addr_delta }
        } else {
            // Move the `None` LEFT to `first_left`: elements A..phys-1 shift
            // right by 1 (each phys += 1 → virt += 1<<addr_shift). The `None`
            // propagates phys → A via adjacent swaps (reversed order).
            let a = first_left as usize;
            for i in ((a + 1)..=phys).rev() {
                block.buf.swap(i, i - 1);
            }
            let minus = 1isize;
            let addr_delta = minus.checked_shl(block.addr_shift)
                                  .expect("handle_removal: addr_shift >= isize::BITS");
            InsertDelta::Move { new_virt: block.phys_to_virt(a),
                                amount: left_dist,
                                minus,
                                addr_delta }
        }
    }

    /// Graduation seam: called after every successful insert. Step 3 plugs in
    /// the `len == half_ptr` pluripotent→concrete specialization here (which
    /// simultaneously rewrites `addr_shift`/`none_mask`/`virt_offset`/`budget`).
    /// Returns `None` if no graduation occurred, or `Some(InsertDelta)` with
    /// remap info.
    ///
    /// Step 2: no-op stub (returns `None`). The pluripotent override (Step 3)
    /// lives at the `PluripotentStrategy` impl — see the `try_insert_at`
    /// dispatch TODO for the variant-change + stale-new_virt design gaps
    /// graduation must address.
    fn post_insert_check(&mut self, _block: &mut Block<T, PTR, OP>) -> Option<InsertDelta<T>> {
        None
    }
}

// -----------------------------------------------------------------------
// Strategy structs
// -----------------------------------------------------------------------

/// Pluripotent strategy — balanced growth in both directions. Budget = `cap`
/// (the whole block — per notes, "budget is the entire block so it can't
/// fail"). Graduates to a concrete strategy at `len == half_ptr`.
pub(crate) struct PluripotentStrategy {
    budget: usize,
}

/// Append strategy — optimized for `push_back`. Budget = `INSERT_BUDGET`.
/// `none_mask = 15` (stride-16 AP). `push_back` inserts `None` gaps to keep the
/// AP stocked (Step 3).
pub(crate) struct AppendStrategy {
    budget: usize,
}

/// Prepend strategy — mirror of append. Budget = `INSERT_BUDGET`.
pub(crate) struct PrependStrategy {
    budget: usize,
}

/// Random strategy — full-range addressing. Budget = `cap`.
pub(crate) struct RandomStrategy {
    budget: usize,
}

// -----------------------------------------------------------------------
// Initializers — strategy structs own the layout formula + budget.
// -----------------------------------------------------------------------

impl PluripotentStrategy {
    /// Construct a new pluripotent block with capacity `cap`.
    ///
    /// `addr_shift = log2(half_ptr) - log2(cap)` so `cap * 2^addr_shift =
    /// half_ptr`: the block always spans exactly `half_ptr` addresses, centered
    /// in the address range (`front_virt = addr_min + (MAX - half_ptr)/2`).
    /// `none_mask = 3` (stride 4). Budget = `cap`.
    pub(crate) fn new_block<T, PTR: SignedBlockIndex, const OP: bool>(cap: usize)
                                                                      -> Block<T, PTR, OP> {
        Block::<T, PTR, OP>::assert_capacity();
        Block::<T, PTR, OP>::assert_cap_pow2(cap);
        let max = Block::<T, PTR, OP>::max_magnitude();
        let half = Block::<T, PTR, OP>::half_ptr();
        assert!(cap <= half, "pluripotent cap {cap} exceeds half_ptr {half}");
        let (addr_min, _) = Block::<T, PTR, OP>::addr_range();
        let addr_shift = half.trailing_zeros() - cap.trailing_zeros();
        let front_virt = addr_min + ((max - half) / 2) as isize;
        Block::build_with_strategy(addr_shift,
                                   3,
                                   -front_virt,
                                   BlockStrategy::Pluripotent(PluripotentStrategy { budget: cap }))
    }
}

impl AppendStrategy {
    /// Construct a new append block with capacity `cap`.
    ///
    /// `addr_shift = 0` (dense). `none_mask = 15` (stride 16). Front at
    /// `addr_min + half_ptr` (low — `half_ptr` reserved below). Budget =
    /// `INSERT_BUDGET`. `cap` is validated but unused in the address math
    /// (reserved for Phase 2 realloc/spread sizing).
    pub(crate) fn new_block<T, PTR: SignedBlockIndex, const OP: bool>(cap: usize)
                                                                      -> Block<T, PTR, OP> {
        Block::<T, PTR, OP>::assert_capacity();
        Block::<T, PTR, OP>::assert_cap_pow2(cap);
        let half = Block::<T, PTR, OP>::half_ptr();
        let (addr_min, _) = Block::<T, PTR, OP>::addr_range();
        let front_virt = addr_min + half as isize;
        Block::build_with_strategy(0,
                                   15,
                                   -front_virt,
                                   BlockStrategy::Append(AppendStrategy { budget: INSERT_BUDGET }))
    }
}

impl PrependStrategy {
    /// Construct a new prepend block with capacity `cap`.
    ///
    /// `addr_shift = 0` (dense). `none_mask = 15` (stride 16). Front at
    /// `addr_max - half_ptr` (high — `half_ptr` reserved above). Budget =
    /// `INSERT_BUDGET`. Mirror of [`AppendStrategy::new_block`].
    pub(crate) fn new_block<T, PTR: SignedBlockIndex, const OP: bool>(cap: usize)
                                                                      -> Block<T, PTR, OP> {
        Block::<T, PTR, OP>::assert_capacity();
        Block::<T, PTR, OP>::assert_cap_pow2(cap);
        let half = Block::<T, PTR, OP>::half_ptr();
        let (_, addr_max) = Block::<T, PTR, OP>::addr_range();
        let front_virt = addr_max - half as isize;
        Block::build_with_strategy(0,
                                   15,
                                   -front_virt,
                                   BlockStrategy::Prepend(PrependStrategy { budget: INSERT_BUDGET, }))
    }
}

impl RandomStrategy {
    /// Construct a new random block with capacity `cap`.
    ///
    /// `addr_shift = log2(MAX) - log2(cap)` so `cap * 2^addr_shift = MAX`: the
    /// block spans the full address range. `none_mask = 1` (stride 2). Front at
    /// `addr_min` — the window IS the full range. Budget = `cap`.
    pub(crate) fn new_block<T, PTR: SignedBlockIndex, const OP: bool>(cap: usize)
                                                                      -> Block<T, PTR, OP> {
        Block::<T, PTR, OP>::assert_capacity();
        Block::<T, PTR, OP>::assert_cap_pow2(cap);
        let max = Block::<T, PTR, OP>::max_magnitude();
        assert!(cap <= max, "random cap {cap} exceeds MAX {max}");
        let (addr_min, _) = Block::<T, PTR, OP>::addr_range();
        let addr_shift = max.trailing_zeros() - cap.trailing_zeros();
        Block::build_with_strategy(addr_shift,
                                   1,
                                   -addr_min,
                                   BlockStrategy::Random(RandomStrategy { budget: cap }))
    }
}

// -----------------------------------------------------------------------
// GrowthStrategy impls
// -----------------------------------------------------------------------

/// Shared insert handler. `Found::At` → place into the on-AP `None` (no shift
/// — the block layer doesn't shift-to-create on insert; `NotFound` → caller
/// splits, Step 4). `Found::Append` → `push_back`, with **gap-insertion** when
/// `gap_on_append` is set (Append/Prepend strategies, stride-16 AP — keeps the
/// AP stocked so `find_slot` is fast; Pluripotent/Random pass `false` since
/// their small stride would waste 25–50% of capacity on gaps). `Found::Prepend`
/// → plain `push_front` (TODO: prepend gap-insertion symmetry).
fn default_handle_insertion<T, PTR: SignedBlockIndex, const OP: bool>(
    block: &mut Block<T, PTR, OP>,
    found: Found,
    _hint_phys: usize,
    value: T,
    gap_on_append: bool)
    -> Result<InsertDelta<T>, NotFound> {
    match found {
        Found::At(phys) => {
            // Reuse the on-AP `None` `find_slot` found within budget — place
            // directly, no shift. (Block layer doesn't shift-to-create on
            // insert; `NotFound` → caller/arena splits, Step 4.)
            block.buf[phys] = Some(value);
            Ok(InsertDelta::Free { new_virt: block.phys_to_virt(phys) })
        }
        Found::Append => {
            // Gap-insertion: keep the AP stocked. If the next back slot
            // (`buf.len()`) is an aligned (AP) position, push a `None` vacancy
            // there first so values never occupy aligned slots — one `None`
            // every `none_stride` keeps `find_slot` fast. Eligibility uses the
            // same spread-frame test as `find_slot`/`align`:
            // `(p + v_off_phys) & none_mask == 1`.
            if gap_on_append {
                let none_mask = block.none_mask as usize;
                let v_off_phys = (block.virt_offset as usize) >> block.addr_shift;
                let next = block.buf.len();
                let gapped = next.wrapping_add(v_off_phys) & none_mask == 1;
                if gapped {
                    block.push_back_none()?;
                }
                return match block.push_back(value) {
                    Ok(new_virt) => Ok(InsertDelta::Free { new_virt }),
                    Err(e) => {
                        // Roll back the gap so `Err` = no mutation.
                        if gapped {
                            block.buf.pop_back();
                        }
                        Err(e)
                    }
                };
            }
            let new_virt = block.push_back(value)?;
            Ok(InsertDelta::Free { new_virt })
        }
        Found::Prepend => {
            // TODO Step 3b (prepend gap-insertion): mirror of the Append arm.
            // Subtle: `push_front` always lands the value at phys 0, which may
            // itself be the aligned slot (its spread-frame coord is
            // `v_off_phys + 1` after the bump). The likely ordering is
            // `push_front(value)` THEN `push_front(None)` so the aligned phys 0
            // becomes the vacancy and the value sits at phys 1 — but the double
            // `virt_offset` bump + the returned `new_virt` (value at phys 1)
            // need address-stability verification before this lands. Plain
            // `push_front` for now.
            let new_virt = block.push_front(value)?;
            Ok(InsertDelta::Free { new_virt })
        }
    }
}

impl<T, PTR: SignedBlockIndex, const OP: bool> GrowthStrategy<T, PTR, OP> for PluripotentStrategy {
    fn handle_insertion(&mut self,
                        block: &mut Block<T, PTR, OP>,
                        found: Found,
                        hint_phys: usize,
                        value: T)
                        -> Result<InsertDelta<T>, NotFound> {
        // No gap-insertion (stride-4 AP — gap would waste 25% of capacity).
        // Step 3: spread/split on NotFound is the arena's job (Step 4).
        default_handle_insertion(block, found, hint_phys, value, false)
    }

    // `handle_removal` is the shared shift-to-AP default (Step 3b);
    // `post_insert_check` uses the no-op graduation default (Step 3).
    // Step 3: override post_insert_check here for pluripotent→concrete
    // graduation (len == half_ptr → rewrite addr_shift/none_mask/virt_offset/
    // budget, return the remap delta). See the try_insert_at dispatch TODO for
    // the variant-change + stale-new_virt design gaps graduation must address.
}

impl<T, PTR: SignedBlockIndex, const OP: bool> GrowthStrategy<T, PTR, OP> for AppendStrategy {
    fn handle_insertion(&mut self,
                        block: &mut Block<T, PTR, OP>,
                        found: Found,
                        hint_phys: usize,
                        value: T)
                        -> Result<InsertDelta<T>, NotFound> {
        // Gap-insertion on Append (stride-16 AP, ~6% vacancy — keeps the AP
        // stocked). No insert shift; NotFound → arena splits (Step 4).
        default_handle_insertion(block, found, hint_phys, value, true)
    }

    // `handle_removal` is the shared shift-to-AP default (Step 3b);
    // `post_insert_check` uses the no-op graduation default (Step 3).
}

impl<T, PTR: SignedBlockIndex, const OP: bool> GrowthStrategy<T, PTR, OP> for PrependStrategy {
    fn handle_insertion(&mut self,
                        block: &mut Block<T, PTR, OP>,
                        found: Found,
                        hint_phys: usize,
                        value: T)
                        -> Result<InsertDelta<T>, NotFound> {
        // Gap-insertion flag set (prepend gap-insertion symmetry is TODO in
        // default_handle_insertion's Prepend arm; the flag is honored once
        // that lands). No insert shift; NotFound → arena splits (Step 4).
        default_handle_insertion(block, found, hint_phys, value, true)
    }

    // `handle_removal` is the shared shift-to-AP default (Step 3b);
    // `post_insert_check` uses the no-op graduation default (Step 3).
}

impl<T, PTR: SignedBlockIndex, const OP: bool> GrowthStrategy<T, PTR, OP> for RandomStrategy {
    fn handle_insertion(&mut self,
                        block: &mut Block<T, PTR, OP>,
                        found: Found,
                        hint_phys: usize,
                        value: T)
                        -> Result<InsertDelta<T>, NotFound> {
        // No gap-insertion (stride-2 AP — gap would waste 50% of capacity).
        // NotFound → arena splits (Step 4).
        default_handle_insertion(block, found, hint_phys, value, false)
    }

    // `handle_removal` is the shared shift-to-AP default (Step 3b);
    // `post_insert_check` uses the no-op graduation default (Step 3).
}
