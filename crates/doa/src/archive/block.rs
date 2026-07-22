use crate::find_slot::*;
use crate::index::{self, *};
use crate::strategy::{AppendStrategy, BlockStrategy, InsertDelta, PluripotentStrategy,
                      PrependStrategy, RandomStrategy};
use std::collections::VecDeque;
use std::marker::PhantomData;

pub struct Block<T, PTR: SignedBlockIndex = i16, const OVERP: bool = false>
    where T: Sized
{
    pub(crate) buf:         VecDeque<Option<T>>,
    pub(crate) prev:        Option<usize>,
    pub(crate) next:        Option<usize>,
    pub(crate) addr_shift:  u32,
    pub(crate) none_mask:   u32,
    pub(crate) virt_offset: isize,
    pub(crate) strategy:    BlockStrategy,
    _phantom:               PhantomData<PTR>,
}

///overp=overprovisioned. i16 blocks will have the capacity of an i8 block, but in exchange
///don't have to care about allocation strategies for efficient address space usage.
impl<T, PTR: SignedBlockIndex, const OVERP: bool> Block<T, PTR, OVERP> where T: Sized
{
    /// `PTR` must be strictly narrower than `isize` (the address-arithmetic
    /// type): reaching `-PTR::MIN` requires `v_offset = PTR::MAX + 1`, which
    /// must not overflow the arithmetic. `isize`/`i64`/`i128` are therefore not
    /// valid `PTR`s. Referenced in every constructor (`from_raw_parts` +
    /// `build_with_strategy`) so it is actually evaluated — an unreferenced
    /// const is never checked.
    const _PTR_NARROWER_THAN_ARITH: () = assert!(
        std::mem::size_of::<PTR>() < std::mem::size_of::<isize>(),
        "PTR must be narrower than isize so PTR::MAX + 1 is representable in the arithmetic type"
    );

    #[inline]
    pub(crate) fn addr_range() -> (isize, isize) {
        if OVERP {
            let root = Self::ptr_root() as isize;
            (-root, root)
        } else {
            let max_addr = PTR::MAX().as_isize();
            (-max_addr, max_addr)
        }
    }

    //max value of a pointer half the size of PTR
    pub(crate) fn ptr_root() -> usize {
        //i8::MAX=127 , >> 1 = 63, + 1 = 64, >>3 = 8, -1 = 7
        return (PTR::MAX().as_isize() >> 1 + 1 >> (PTR::bit_width() as u32 / 2 - 1) - 1) as usize;
    }

    pub(crate) fn from_raw_parts(buf: VecDeque<Option<T>>,
                                 addr_shift: u32,
                                 none_mask: u32,
                                 virt_offset: isize)
                                 -> Self {
        let () = Self::_PTR_NARROWER_THAN_ARITH;
        Self { buf,
               prev: None,
               next: None,
               addr_shift,
               none_mask,
               virt_offset,
               strategy: BlockStrategy::default(),
               _phantom: PhantomData }
    }

    // -----------------------------------------------------------------
    // Strategy initializers
    // set addr_shift, none_mask, virt_offset, to optimize the block for
    // a particular workload- particularly, its use of address and physical
    // space.
    // -----------------------------------------------------------------

    /// Shared build tail for the strategy initializers.
    pub(crate) fn build_with_strategy(addr_shift: u32,
                                      none_mask: u32,
                                      virt_offset: isize,
                                      strategy: BlockStrategy)
                                      -> Self {
        let () = Self::_PTR_NARROWER_THAN_ARITH;
        Self { buf: VecDeque::new(),
               prev: None,
               next: None,
               addr_shift,
               none_mask,
               virt_offset,
               strategy,
               _phantom: PhantomData }
    }

    /// Pluripotent profile — balanced growth in both directions.
    /// `cap` must be a power of two and `<= half_ptr`.
    pub fn new_pluripotent(cap: usize) -> Self {
        PluripotentStrategy::new_block(cap)
    }

    /// Random profile — full-range addressing.
    pub fn new_random(cap: usize) -> Self {
        RandomStrategy::new_block(cap)
    }

    /// Append profile — optimized for `push_back`; reserves `half_ptr`
    /// addresses below for `push_front`.
    pub fn new_append(cap: usize) -> Self {
        AppendStrategy::new_block(cap)
    }

    /// Prepend profile — optimized for `push_front`; reserves `half_ptr`
    /// addresses above for `push_back`.
    pub fn new_prepend(cap: usize) -> Self {
        PrependStrategy::new_block(cap)
    }

    /// Set the scan budget (test/bench convenience). Returns `self` for
    /// chaining. The budget lives on the strategy struct; this reaches through
    /// to set it. The graduation seam (Step 3) will also use this to change
    /// budget on pluripotent→concrete specialization.
    #[must_use]
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.strategy.set_budget(budget);
        self
    }

    /// Virtual address → physical buffer index.
    #[inline]
    pub fn virt_to_phys(&self, virt: isize) -> usize {
        return ((virt + self.virt_offset) >> self.addr_shift) as usize;
    }

    /// Physical buffer index → virtual address.
    #[inline]
    pub fn phys_to_virt(&self, phys: usize) -> isize {
        virt_of(phys, self.addr_shift, self.virt_offset as usize)
    }

    /// Locate the nearest reusable `None` slot (or a viable end) for an
    /// insert at physical index `anchor`.
    ///
    /// Precomputes each side's in-range eligible-slot count and caps it at
    /// `budget`, then probes outward in two phases: phase 1 walks both sides in
    /// lockstep for the shared count (near side first, per `right_first`),
    /// phase 2 drains the longer side's remainder.
    pub fn find_slot(&self, anchor: usize, bias: Bias) -> Result<Found, NotFound> {
        let budget = self.strategy.budget();
        // `v_offset` is the canonical conversion (wrapping `as usize`);
        // `v_off_phys` derives from it by an UNSIGNED shift. The scan hot path
        // touches only the low `none_mask` bits of `v_off_phys`
        let v_offset = self.virt_offset as usize;
        let v_off_phys = v_offset >> self.addr_shift;
        let (addr_min, addr_max) = Self::addr_range();

        let len = self.buf.len();
        let len_isize = len as isize;
        let (front, back) = self.buf.as_slices();
        let slots = Slots { front, back };
        let scanp = ScanParameters::new(anchor, self.none_mask as usize, v_off_phys, bias);
        let first_right = scanp.first_right;
        let first_left = scanp.first_left;
        let stride = scanp.stride;

        let (count_right, count_left) = scanp.counts(len_isize);
        // Probes actually performed per side, capped at budget.
        let probes_right = count_right.min(budget);
        let probes_left = count_left.min(budget);
        let mut pos_right = first_right;
        let mut pos_left = first_left;

        // Phase 1: both sides for the shared count, near side first. 
        // Every probe position stays in
        // `[0, len)` (probes capped to in-range counts), so `at_unchecked` is
        // sound.
        let shared = probes_right.min(probes_left);
        if scanp.right_first {
            for _ in 0..shared {
                if unsafe { slots.at_unchecked(pos_right as usize) }.is_none() {
                    return Ok(Found::At(pos_right as usize));
                }
                pos_right += stride;
                if unsafe { slots.at_unchecked(pos_left as usize) }.is_none() {
                    return Ok(Found::At(pos_left as usize));
                }
                pos_left -= stride;
            }
        } else {
            for _ in 0..shared {
                if unsafe { slots.at_unchecked(pos_left as usize) }.is_none() {
                    return Ok(Found::At(pos_left as usize));
                }
                pos_left -= stride;
                if unsafe { slots.at_unchecked(pos_right as usize) }.is_none() {
                    return Ok(Found::At(pos_right as usize));
                }
                pos_right += stride;
            }
        }
        // Phase 2: drain whichever side still has probes left (at most one —
        // `shared` is the smaller cap, so only the larger side has a remainder; the
        // other call gets a zero count and compiles away).
        if let Some(slot) = scan(&slots, pos_right, stride, probes_right - shared) {
            return Ok(Found::At(slot));
        }
        if let Some(slot) = scan(&slots, pos_left, -stride, probes_left - shared) {
            return Ok(Found::At(slot));
        }

        resolve_ends(bias,
                     count_right,
                     count_left,
                     budget,
                     self.addr_shift,
                     v_offset,
                     addr_min,
                     addr_max,
                     len)
    }

    /// Read the value at virtual address `virt`. Translates via
    /// [`virt_to_phys`](Block::virt_to_phys) and indexes the buffer.
    ///
    /// **Debug-asserts** (panics in debug, unchecked in release) that `virt` is
    /// the live address of an occupied slot: `phys` in range (`< buf.len()`)
    /// and the slot is `Some` (a use-after-remove is a consumer bug). Returns
    /// `&T` directly — a live address always has a value, so there is no `None`
    /// case to represent.
    #[inline]
    pub fn get(&self, virt: isize) -> &T {
        let phys = self.virt_to_phys(virt);
        debug_assert!(phys < self.buf.len(),
                      "get: {virt} -> phys {phys} OOB (len {})",
                      self.buf.len());
        let slot = &self.buf[phys];
        debug_assert!(slot.is_some(), "get: address {virt} is stale (slot unoccupied)");
        // SAFETY: occupied by the debug_assert above.
        unsafe { slot.as_ref().unwrap_unchecked() }
    }

    /// Mutable read the value at virtual address `virt`. See [`get`](Block::get)
    /// for the debug-assert contract (stale/garbage/OOB/unoccupied virt →
    /// debug-panic). Returns `&mut T` directly.
    #[inline]
    pub fn get_mut(&mut self, virt: isize) -> &mut T {
        let phys = self.virt_to_phys(virt);
        debug_assert!(phys < self.buf.len(),
                      "get_mut: {virt} -> phys {phys} OOB (len {})",
                      self.buf.len());
        let slot = &mut self.buf[phys];
        debug_assert!(slot.is_some(), "get_mut: address {virt} is stale (slot unoccupied)");
        // SAFETY: occupied by the debug_assert above.
        unsafe { slot.as_mut().unwrap_unchecked() }
    }

    // ---------------------------------------------------------------------
    // Mutation surface. `push_back`/`push_front` only touch the
    // phys↔virt translation (no strategy dispatch). `try_insert_*`
    // and `remove` return [`InsertDelta`] for caller-side pointer
    // remap.
    // ---------------------------------------------------------------------

    /// Would a `push_back` (new slot at `phys == len`) hand out a representable
    /// address? Thin wrapper over `find_slot::back_viable` — the formula lives
    /// in one place; this supplies `addr_max` from `self.addr_range()`.
    #[inline]
    fn back_viable(&self) -> bool {
        let (_, addr_max) = Self::addr_range();
        crate::find_slot::back_viable(self.buf.len(),
                                      self.addr_shift,
                                      self.virt_offset as usize,
                                      addr_max)
    }

    /// Would a `push_front` (new slot at `phys 0`, `virt_offset` bumped) hand
    /// out a representable address? `push_front` bumps `virt_offset` by
    /// `1 << addr_shift` (one phys slot's worth of virt addresses), so the new
    /// front address is `-(virt_offset + (1 << addr_shift))`. This is stable for
    /// ALL `addr_shift` (the bump cancels the phys shift for existing elements),
    /// not just 0. Thin wrapper over `find_slot::front_viable`.
    #[inline]
    fn front_viable(&self) -> bool {
        let (addr_min, _) = Self::addr_range();
        crate::find_slot::front_viable(self.addr_shift, self.virt_offset as usize, addr_min)
    }

    /// Append `value` at the back of the buffer. Address-stable. Returns the new element's virt
    /// address, or `Err(AddressExhaustion)` if `phys_to_virt(len)` is outside
    /// `PTR`'s representable range.
    pub fn push_back(&mut self, value: T) -> Result<isize, NotFound> {
        if !self.back_viable() {
            return Err(NotFound::AddressExhaustion);
        }
        self.buf.push_back(Some(value));
        // SAFETY: `buf.len()` was `>= 0` before push; now `>= 1`.
        Ok(self.phys_to_virt(self.buf.len() - 1))
    }

    /// Prepend `value` at the front of the buffer. Address-stable for ALL
    /// `addr_shift`: `buf.push_front` shifts every existing element's phys up
    /// by 1, and bumping `virt_offset` by `1 << addr_shift` (one slot's worth of
    /// virt addresses) exactly cancels that shift for every existing element —
    /// `phys_to_virt(p+1)` with the new offset equals `phys_to_virt(p)` with the
    /// old. The new front lands at `-(virt_offset + (1 << addr_shift))`, leaving
    /// `(1<<shift)-1` unallocated addresses below the old front (the spread gap
    /// pattern). Returns the new front's virt address, or
    /// `Err(AddressExhaustion)` if that address is outside `PTR`'s range.
    pub fn push_front(&mut self, value: T) -> Result<isize, NotFound> {
        if !self.front_viable() {
            return Err(NotFound::AddressExhaustion);
        }
        self.buf.push_front(Some(value));
        debug_assert!(1isize.checked_shl(self.addr_shift).is_some(),
                      "push_front: addr_shift >= isize::BITS");
        let step = 1isize << self.addr_shift;
        debug_assert!(self.virt_offset.checked_add(step).is_some(),
                      "push_front: virt_offset overflow despite front_viable gate");
        self.virt_offset += step;
        Ok(self.phys_to_virt(0))
    }

    /// Append a `None` vacancy at the back (gap-insertion: stock the AP with
    /// aligned `None`s so `find_slot` finds a slot quickly). Viability-checked
    /// like `push_back` — the gap occupies a phys slot and consumes address
    /// space even though its address isn't handed out. No value to return.
    pub(crate) fn push_back_none(&mut self) -> Result<(), NotFound> {
        if !self.back_viable() {
            return Err(NotFound::AddressExhaustion);
        }
        self.buf.push_back(None);
        Ok(())
    }

    /// Prepend a `None` vacancy at the front (gap-insertion: stock the AP with
    /// aligned `None`s so `find_slot` finds a slot quickly). Viability-checked
    /// like `push_back` — the gap occupies a phys slot and consumes address
    /// space even though its address isn't handed out.
    pub(crate) fn push_front_none(&mut self) -> Result<(), NotFound> {
        if !self.front_viable() {
            return Err(NotFound::AddressExhaustion);
        }
        self.buf.push_front(None);
        Ok(())
    }

    /// Remove the value at virtual address `virt` (set the slot to `None`).
    /// Returns `(removed_value, InsertDelta)` — the value, plus the delta
    /// describing any element movement the strategy performed (Step 2: always
    /// `InsertDelta::Free`, no moves; Step 3 plugs in shift-to-AP → `Move`).
    ///
    /// **Debug-asserts** (panics in debug, unchecked in release) that `virt` is
    /// the live address of an occupied slot: `phys` in range, and the slot is
    /// not already `None` (double-remove / use-after-remove is a consumer bug).
    /// The value is therefore always present — returned as `T`, not
    /// `Option<T>`.
    ///
    /// Dispatches through `self.strategy.handle_removal(self, phys)` — the
    /// strategy's removal seam. Callers that only need the value extract `.0`;
    /// callers that need to remap pointers consume the `InsertDelta`.
    pub fn remove(&mut self, virt: isize) -> (T, InsertDelta<T>) {
        let phys = self.virt_to_phys(virt);
        debug_assert!(phys < self.buf.len(),
                      "remove: {virt} -> phys {phys} OOB (len {})",
                      self.buf.len());
        debug_assert!(self.buf[phys].is_some(),
                      "remove: double free at v_address {virt}");
        // SAFETY: occupied by the debug_assert above.
        let value = unsafe { self.buf[phys].take().unwrap_unchecked() };
        // Swap the strategy out to avoid the split-borrow (&mut strategy +
        // &mut Block), dispatch the removal seam, then swap back. The strategy
        // is a standalone value during the call; the block's `strategy` field
        // temporarily holds the Default placeholder.
        //
        // TODO Step 3 (panic-safety): if `handle_removal` panics once it indexes
        // `buf` for the shift, the swap-back never runs and `self.strategy` is
        // left as the `Default` placeholder — the real strategy is lost. An
        // RAII guard that restores the strategy on `Drop` fixes it; not needed
        // yet (Step 2's removal is pure arithmetic — no panic surface).
        let mut strategy = &mut self.strategy;
        let delta = strategy.handle_removal(self, phys);
        self.strategy = strategy;
        (value, delta)
    }

    fn try_insert_at(&mut self,
                     anchor_phys: usize,
                     value: T,
                     bias: Bias)
                     -> Result<InsertDelta<T>, NotFound> {
        let found = self.find_slot(anchor_phys, bias)?;
        let mut strategy = std::mem::take(&mut self.strategy);
        let result = strategy.handle_insertion(self, found, anchor_phys, value);
        // Graduation seam (no-op Step 2). TODO Step 3 — three coupled gaps to
        // solve before graduation works:
        // (a) Variant change: graduation flips the `BlockStrategy` variant, but
        //     `self.strategy = strategy` below restores the OLD one.
        //     `post_insert_check` has `&mut self` (inner struct) + `&mut Block`,
        //     no tag handle — it must RETURN the new `BlockStrategy` to install.
        // (b) Stale `new_virt`: `result.new_virt` used the OLD addr_shift/
        //     virt_offset; graduation rewrites them. Recompute after graduation,
        //     or merge a `Graduate` variant into `result` (`Move` can't describe
        //     a per-phys address change — see its TODO).
        // (c) Panic-safety: if `post_insert_check` panics once non-trivial,
        //     `strategy` is lost to `Default` (see `remove`).
        let _graduation_delta =
            if result.is_ok() { strategy.post_insert_check(self) } else { None };
        self.strategy = strategy;
        // TODO Step 3: consume `_graduation_delta` (install graduated variant +
        // fix up `result`). Discarded for Step 2 (always `None`).
        result
    }

    pub fn try_insert_before(&mut self,
                             anchor_phys: usize,
                             value: T)
                             -> Result<InsertDelta<T>, NotFound> {
        self.try_insert_at(anchor_phys, value, Bias::Left)
    }

    pub fn try_insert_after(&mut self,
                            anchor_phys: usize,
                            value: T)
                            -> Result<InsertDelta<T>, NotFound> {
        self.try_insert_at(anchor_phys, value, Bias::Right)
    }

    // TODO (separate tasks): spread / iter / split.
    // (removal shift-to-AP landed in Step 3b via `handle_removal`.)

    /// Test-only constructor: delegates to [`from_raw_parts`]. Kept as a
    /// named shortcut so existing tests don't all change call sites.
    #[cfg(test)]
    fn test_new(buf: VecDeque<Option<T>>,
                addr_shift: u32,
                none_mask: u32,
                virt_offset: isize)
                -> Self {
        Self::from_raw_parts(buf, addr_shift, none_mask, virt_offset)
    }
}

impl<T, PTR: SignedBlockIndex, const OVERP: bool> Default for Block<T, PTR, OVERP> where T: Sized
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `phys_to_virt(virt_to_phys(v)) == v` holds when `v` is aligned to
    /// `2^addr_shift` (i.e. `v + virt_offset` is a multiple of the stride).
    /// Generate aligned `v` via `v = p·stride − virt_offset`.
    #[test]
    fn roundtrip_virt_to_phys_to_virt() {
        for &addr_shift in &[0u32, 1, 2, 4, 8] {
            for &v_off in &[0isize, 1, -1, 3, -7, 100, -100] {
                let mut block: Block<u64, i32> = Block::new();
                block.addr_shift = addr_shift;
                block.virt_offset = v_off;
                let stride = 1isize << addr_shift;
                for p in 0..32isize {
                    let v = p * stride - v_off;
                    let phys = block.virt_to_phys(v);
                    assert_eq!(block.phys_to_virt(phys),
                               v,
                               "virt→phys→virt failed: shift={addr_shift} v_off={v_off} v={v} phys={phys}",);
                }
            }
        }
    }

    /// `virt_to_phys(phys_to_virt(p)) == p` holds for every in-range `phys`
    /// regardless of alignment — the dense index is the round-trip identity.
    #[test]
    fn roundtrip_phys_to_virt_to_phys() {
        for &addr_shift in &[0u32, 1, 2, 4, 8] {
            for &v_off in &[0isize, 1, -1, 3, -7] {
                let mut block: Block<u64, i32> = Block::new();
                block.addr_shift = addr_shift;
                block.virt_offset = v_off;
                for p in 0..64usize {
                    let v = block.phys_to_virt(p);
                    assert_eq!(block.virt_to_phys(v),
                               p,
                               "phys→virt→phys failed: shift={addr_shift} v_off={v_off} p={p}",);
                }
            }
        }
    }

    /// `get` returns the value placed at a known physical index. Uses
    /// `addr_shift = 0`, `virt_offset = 0` so `virt == phys` and the translation
    /// is the identity, isolating the `buf` lookup. A live address always yields
    /// `&T`; the stale/OOB/empty-slot cases are covered by the `*_panics` tests.
    #[test]
    fn get_returns_placed_value() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 0;
        block.virt_offset = 0;
        block.buf.push_back(Some(42));
        block.buf.push_back(Some(7));
        block.buf.push_back(None);
        block.buf.push_back(Some(99));
        assert_eq!(block.get(0), &42);
        assert_eq!(block.get(1), &7);
        assert_eq!(block.get(3), &99);
    }

    #[test]
    fn get_mut_updates_value() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 0;
        block.virt_offset = 0;
        block.buf.push_back(Some(10));
        *block.get_mut(0) = 20;
        assert_eq!(block.get(0), &20);
    }

    /// `get` with `addr_shift = 1`: a live virt is slot-aligned (a multiple of
    /// the stride), so `virt 4` maps to phys 2. An *unaligned* virt (e.g. 5) is
    /// not a live address — the old code silently dropped the low bit and mapped
    /// it to phys 2 anyway; the new contract rejects it (see
    /// `get_on_unaligned_virt_panics`).
    #[test]
    fn get_with_shift_drops_low_bit() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 1;
        block.virt_offset = 0;
        for _ in 0..5 {
            block.buf.push_back(None);
        }
        block.buf[2] = Some(123);
        assert_eq!(block.get(4), &123); // virt 4 → phys 2 (aligned)
    }

    /// A `get` on a virt that isn't slot-aligned (the low bits below
    /// `addr_shift` are nonzero) is a stale/garbage ptr → panic. The translation
    /// no longer silently drops the low bit to a neighboring slot.
    #[test]
    #[should_panic(expected = "not slot-aligned")]
    fn get_on_unaligned_virt_panics() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 1;
        block.virt_offset = 0;
        for _ in 0..5 {
            block.buf.push_back(None);
        }
        block.buf[2] = Some(123);
        let _ = block.get(5); // virt 5 is unaligned (stride 2) — not a live address
    }

    /// A `get` on an unoccupied (gap/removed) slot is a stale-ptr bug → panic.
    #[test]
    #[should_panic(expected = "stale (slot unoccupied)")]
    fn get_on_empty_slot_panics() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 0;
        block.virt_offset = 0;
        block.buf.push_back(Some(42));
        block.buf.push_back(None); // phys 1 unoccupied
        let _ = block.get(1);
    }

    /// A `get` on a virt that translates OOB (`phys >= len`) → panic.
    #[test]
    #[should_panic(expected = "OOB")]
    fn get_oob_panics() {
        let mut block: Block<u64, i16> = Block::new();
        block.addr_shift = 0;
        block.virt_offset = 0;
        block.buf.push_back(Some(42));
        let _ = block.get(4); // phys 4 >= len 1
    }

    // ---------------------------------------------------------------------
    // Differential test: Block::find_slot vs an
    // independent pure-isize oracle. Covers the cases the 8000-case
    // `find_slot::tests::matches_reference` misses: negative `virt_offset`
    // (split-off-block case), `addr_shift > 0`, `len == 0`, `anchor == len`,
    // and `none_mask == 0` (degenerate stride-1). Regression-protects the
    // solo-probe guard in `Block::find_slot`.
    // ---------------------------------------------------------------------

    use crate::find_slot::{Bias, Found, NotFound};

    /// Deterministic xorshift (same shape as `find_slot::tests::lcg`).
    fn lcg(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Independent pure-`isize` oracle. Recomputes the eligible-slot AP from the
    /// mask formula with an ARITHMETIC shift (production uses an unsigned-shift
    /// derivation; the two agree in the low `none_mask` bits, so this is a
    /// genuine independent check), collects in-budget candidates on both sides
    /// of `anchor`, sorts by distance with a bias-governed tie-break, returns
    /// the first `None`; else resolves ends with first-principles viability
    /// checks. Does NOT touch `align`/`resolve`/`End`.
    fn oracle(buf: &[Option<u64>],
              addr_shift: u32,
              none_mask: u32,
              virt_offset: isize,
              budget: usize,
              anchor: usize,
              addr_min: isize,
              addr_max: isize,
              bias: Bias)
              -> Result<Found, NotFound> {
        let len = buf.len();
        let len_isize = len as isize;
        let stride = (none_mask as isize) + 1;
        let nm = none_mask as isize;
        // Arithmetic shift — the conceptually-signed frame shift.
        let frame = virt_offset >> addr_shift;

        // Eligibility from first principles (independent sanity check only;
        // candidate generation below uses the AP geometry, which for the
        // degenerate `none_mask == 0` case is "every slot" despite the mask
        // formula yielding no eligible slots).
        let eligible = |p: usize| -> bool { (((p as isize) + frame) & nm) == 1 };

        // first_right: first p >= anchor on the eligible AP. Derived from the
        // mask directly, not via `align`.
        let residue = ((anchor as isize + frame) & nm) as usize;
        let right_delta = (1usize.wrapping_sub(residue)) & none_mask as usize;
        let first_right = (anchor + right_delta) as isize;
        let first_left = first_right - stride;

        // Collect in-budget candidates as (dist, pos). Right: ranks 0..budget;
        // left: ranks 1..=budget. dist is the physical |p - anchor|.
        let mut candidates: Vec<(usize, isize)> = Vec::new();
        for rank in 0..budget as isize {
            let pos = first_right + rank * stride;
            if pos >= len_isize {
                break;
            }
            // Independent eligibility sanity (skipped for degenerate none_mask==0
            // where the mask formula and AP geometry disagree by design).
            if none_mask != 0 {
                debug_assert!(eligible(pos as usize),
                              "oracle right elig fail: pos={} frame={} nm={} virt_offset={} shift={}",
                              pos,
                              frame,
                              nm,
                              virt_offset,
                              addr_shift);
            }
            candidates.push(((right_delta as isize + rank * stride) as usize, pos));
        }
        for rank in 1..=budget as isize {
            let pos = first_right - rank * stride;
            if pos < 0 {
                break;
            }
            if none_mask != 0 {
                debug_assert!(eligible(pos as usize), "oracle left elig fail");
            }
            candidates.push(((rank * stride - right_delta as isize) as usize, pos));
        }
        // sort by dist asc; tie-break by bias: Right → greater pos (right
        // side) first, Left → lesser pos (left side) first.
        match bias {
            Bias::Right => {
                candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            }
            Bias::Left => {
                candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            }
        }
        for (_, pos) in candidates {
            if buf[pos as usize].is_none() {
                return Ok(Found::At(pos as usize));
            }
        }

        // Resolve ends. In-range eligible-slot counts per side (uncapped).
        let count_right = if first_right >= len_isize {
            0
        } else {
            ((len_isize - 1 - first_right) / stride + 1) as usize
        };
        let count_left = if first_left < 0 { 0 } else { (first_left / stride + 1) as usize };

        // End viability from first principles.
        // Append: phys_to_virt(len) = (len << addr_shift) - virt_offset <= addr_max.
        let back_virt = ((len as isize) << addr_shift).wrapping_sub(virt_offset);
        let back_viable = back_virt <= addr_max;
        // Prepend: new front address = -(virt_offset + (1 << addr_shift)) >= addr_min.
        // push_front bumps virt_offset by one slot's worth of virt addresses, so
        // it's stable for all addr_shift (not just 0).
        let front_virt = virt_offset.wrapping_add(1isize.wrapping_shl(addr_shift)).wrapping_neg();
        let front_viable = front_virt >= addr_min;

        // Classify each side's end.
        let end_right = if count_right > budget {
            'B'
        } else if back_viable {
            'V'
        } else {
            'A'
        };
        let end_left = if count_left > budget {
            'B'
        } else if front_viable {
            'V'
        } else {
            'A'
        };

        // Combine — mirrors `find_slot::resolve` ordering. Both-ends-viable is
        // bias-controlled (Right→Append/back, Left→Prepend/front); a single
        // viable end is taken regardless of bias.
        match (end_right, end_left) {
            ('V', 'V') => match bias {
                Bias::Right => Ok(Found::Append),
                Bias::Left => Ok(Found::Prepend),
            },
            ('V', _) => Ok(Found::Append),
            (_, 'V') => Ok(Found::Prepend),
            ('A', _) | (_, 'A') => Err(NotFound::AddressExhaustion),
            _ => Err(NotFound::OutOfBudget),
        }
    }

    #[test]
    fn find_slot_matches_oracle() {
        let mut rng = lcg(0x0bad_f00d_c0de_face);
        for _ in 0..12_000 {
            let addr_shift = (rng() % 4) as u32; // 0..=3
            let none_mask = *[0u32, 1, 3, 7, 15].get((rng() % 5) as usize).unwrap();
            // Span NEGATIVE and positive virt_offset (split-off-block case).
            let virt_offset = (rng() % 513) as isize - 256; // -256..=256
            let budget = 1 + (rng() % 20) as usize; // 1..=20
            let len = (rng() % 513) as usize; // 0..=512 (include 0!)
            let anchor = (rng() % (len as u64 + 1)) as usize; // 0..=len (include len!)

            // Deterministic None pattern, independent of eligibility.
            let mut buf_vec: Vec<Option<u64>> = Vec::with_capacity(len);
            let mut fill_rng = lcg(rng());
            for idx in 0..len {
                if fill_rng() % 3 == 0 {
                    buf_vec.push(None);
                } else {
                    buf_vec.push(Some(idx as u64));
                }
            }

            // `Block::find_slot` scans `self.buf` (a `VecDeque`, indexed directly);
            // the oracle scans `buf_vec` independently. Both hold identical contents
            // (the block was built from a clone), so they must agree. Test BOTH
            // biases against the bias-aware oracle.
            for bias in [Bias::Right, Bias::Left] {
                let block: Block<u64, i16> = Block::test_new(VecDeque::from(buf_vec.clone()),
                                                             addr_shift,
                                                             none_mask,
                                                             virt_offset).with_budget(budget);
                let got = block.find_slot(anchor, bias);

                let expected = oracle(&buf_vec,
                                      addr_shift,
                                      none_mask,
                                      virt_offset,
                                      budget,
                                      anchor,
                                      i16::MIN as isize,
                                      i16::MAX as isize,
                                      bias);
                assert_eq!(
                           got,
                           expected,
                           "twophase!=oracle addr_shift={addr_shift} none_mask={none_mask} \
                     virt_offset={virt_offset} budget={budget} len={len} anchor={anchor} \
                     stride={} bias={bias:?} got={got:?} expected={expected:?}",
                           none_mask + 1,
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // Focused unit asserts for the previously-panicking edge cases.
    // ---------------------------------------------------------------------

    #[test]
    fn empty_buf_anchor_zero_returns_append() {
        // len==0, anchor==0, default-ish block: back_virt = 0 - 0 = 0 <= i16::MAX.
        let buf_vec: Vec<Option<u64>> = Vec::new();
        let block: Block<u64, i16> = Block::test_new(VecDeque::from(buf_vec.clone()), 0, 0, 0);
        assert_eq!(block.find_slot(0, Bias::Right),
                   Ok(Found::Append),
                   "empty buf / anchor 0 must return Append, not panic");
    }

    #[test]
    fn anchor_equals_len_returns_append_when_back_viable() {
        // Non-empty block, anchor at the append-at-end hint. back_virt = len <= i16::MAX.
        let len = 8usize;
        let buf_vec: Vec<Option<u64>> = vec![Some(0); len];
        let block: Block<u64, i16> = Block::test_new(VecDeque::from(buf_vec.clone()), 0, 3, 0);
        assert_eq!(block.find_slot(len, Bias::Right),
                   Ok(Found::Append),
                   "anchor==len on back-viable block must return Append, not panic");
    }

    #[test]
    fn anchor_greater_than_len_does_not_ub() {
        // Regression: `anchor > len` left `count_left` uncapped (`Align::counts`
        // only caps `count_right` at `>= len`), so `first_left = len` and the
        // phase-2 left scan started at `pos_left = len` → `at_unchecked(len)`
        // OOB. `anchor == len` is valid (append hint); only `> len` is rejected.
        let buf_vec: Vec<Option<u64>> = vec![Some(0); 8];
        let block: Block<u64, i16> = Block::test_new(VecDeque::from(buf_vec.clone()), 0, 0, 0);
        assert_eq!(block.find_slot(9, Bias::Right),
                   Err(NotFound::OutOfBudget),
                   "anchor > len must be rejected at the API boundary, not UB");
        // Wrap-prone: none_mask=0 makes right_delta==0 for every anchor, the
        // exact geometry that exposed the bug.
        assert_eq!(block.find_slot(100, Bias::Right), Err(NotFound::OutOfBudget),);
    }

    #[test]
    fn none_mask_zero_dense_does_not_panic() {
        // Degenerate stride-1 block: right_delta==0 for every anchor. A None in
        // the buffer should be located without panicking, and end resolution
        // should yield a valid variant.
        let mut buf_vec = vec![Some(0u64); 16];
        buf_vec[5] = None;
        let block: Block<u64, i16> = Block::test_new(VecDeque::from(buf_vec.clone()), 0, 0, 0);
        let res = block.find_slot(0, Bias::Right);
        // Must be a sane variant (At the None, or Append/Prepend) — never panic.
        assert!(matches!(res, Ok(Found::At(_)) | Ok(Found::Append) | Ok(Found::Prepend)),
                "none_mask==0 dense block must return a sane variant, got {res:?}");
        // Concretely: the None at index 5 is the nearest eligible-slot candidate
        // (stride 1, every slot on the AP), so find_slot locates it.
        assert_eq!(res, Ok(Found::At(5)));
    }

    /// `push_front` is address-stable for ALL `addr_shift`, not just 0, because
    /// it bumps `virt_offset` by `1 << addr_shift` (one phys slot's worth of virt
    /// addresses) — the bump exactly cancels the phys shift for every existing
    /// element. Verifies the user's correction to the earlier shift==0-only
    /// design.
    #[test]
    fn push_front_stable_for_nonzero_shift() {
        let mut block: Block<u64, i16> = Block::test_new(
            VecDeque::from(vec![Some(10u64), Some(20), Some(30)]),
            1, // addr_shift = 1
            0,
            0, // virt_offset = 0
        );
        // Existing addresses (shift=1, virt_offset=0): phys 0→0, 1→2, 2→4.
        assert_eq!(block.phys_to_virt(0), 0);
        assert_eq!(block.phys_to_virt(1), 2);
        assert_eq!(block.phys_to_virt(2), 4);

        // push_front: bump virt_offset by 1<<1 = 2; new front at virt -2.
        let new_addr = block.push_front(99).expect("front viable");
        assert_eq!(new_addr, -2);
        assert_eq!(block.get(-2), &99);

        // Every previously-live address still resolves to its original value —
        // the address-stability invariant holds for shift > 0.
        assert_eq!(block.get(0), &10);
        assert_eq!(block.get(2), &20);
        assert_eq!(block.get(4), &30);

        // phys mapping shifted by the push: old phys 0→1, 1→2, 2→3; phys_to_virt
        // reflects the new virt_offset.
        assert_eq!(block.phys_to_virt(1), 0);
        assert_eq!(block.phys_to_virt(2), 2);
        assert_eq!(block.phys_to_virt(3), 4);
    }

    // ---------------------------------------------------------------------
    // Phase-1 mutation surface: focused unit tests + GP1 property test.
    // ---------------------------------------------------------------------

    #[test]
    fn push_back_and_push_front_return_representable_addresses() {
        // addr_shift=0, virt_offset=0: dense, address-stable-for-all-ops config.
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 0);

        let v0 = block.push_back(100).unwrap();
        let v1 = block.push_back(200).unwrap();
        assert_eq!(block.get(v0), &100);
        assert_eq!(block.get(v1), &200);

        let f0 = block.push_front(50).unwrap();
        // After push_front, virt_offset becomes 1; new front addr = -virt_offset = -1.
        assert_eq!(f0, -1);
        assert_eq!(block.get(f0), &50);
        // Address stability: previously-inserted elements still resolve.
        assert_eq!(block.get(v0), &100);
        assert_eq!(block.get(v1), &200);
    }

    #[test]
    fn push_front_refuses_when_front_not_viable() {
        // shift=0, virt_offset=32768: a push_front would hand out
        // -(32768 + 1) = -32769, below i16::MIN → not representable → refuse.
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 32768);
        assert_eq!(block.push_front(1), Err(NotFound::AddressExhaustion));
        // push_back still works on the same block (back is viable on empty buf).
        assert!(block.push_back(1).is_ok());
    }

    /// `remove` returns the value directly (`T`, not `Option<T>`) for a live
    /// address. The stale/double-remove/OOB cases panic — covered below.
    #[test]
    fn remove_returns_value() {
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 0);
        let v = block.push_back(42).unwrap();
        let (removed, _delta) = block.remove(v);
        assert_eq!(removed, 42);
    }

    /// A `get` on an address whose slot was just removed is a use-after-remove
    /// bug → panic.
    #[test]
    #[should_panic(expected = "stale (slot unoccupied)")]
    fn get_after_remove_panics() {
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 0);
        let v = block.push_back(42).unwrap();
        let _ = block.remove(v);
        let _ = block.get(v);
    }

    /// Removing the same address twice is a double-remove bug → panic.
    #[test]
    #[should_panic(expected = "stale (slot already unoccupied)")]
    fn remove_twice_panics() {
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 0);
        let v = block.push_back(42).unwrap();
        let _ = block.remove(v);
        let _ = block.remove(v);
    }

    /// Removing an out-of-range address → panic (consumer passed a garbage ptr).
    #[test]
    #[should_panic(expected = "OOB")]
    fn remove_oob_panics() {
        let mut block: Block<u64, i16> = Block::test_new(VecDeque::new(), 0, 0, 0);
        let _ = block.remove(99999);
    }

    /// Apply a `remove`'s `InsertDelta` to a shadow `virt -> value` map: rekey
    /// the shifted elements by `addr_delta`. `virt` is the removed address (the
    /// shift pivot); for `Move`, the `amount` elements whose phys lay strictly
    /// between `phys` and the aligned target `A` each shifted by one slot, their
    /// virts changing by `addr_delta`. `Free`/`BlockSplit` need no rekey. This is
    /// the caller-side remap `InsertDelta` exists for.
    fn apply_remove_delta<PTR: SignedBlockIndex, const OP: bool>(shadow: &mut std::collections::HashMap<isize, u64>,
                                                                 block: &Block<u64, PTR, OP>,
                                                                 virt: isize,
                                                                 delta: &InsertDelta<u64>) {
        let phys = block.virt_to_phys(virt);
        match delta {
            InsertDelta::Free { .. } => {}
            InsertDelta::BlockSplit { .. } => unreachable!("BlockSplit is Step 4"),
            InsertDelta::Move { new_virt, addr_delta, .. } => {
                let a = block.virt_to_phys(*new_virt);
                // Shifted phys range (OLD phys): the slots strictly between the
                // vacated `phys` and the aligned target `a`.
                let (lo, hi) = if a > phys { (phys + 1, a) } else { (a, phys - 1) };
                let to_rekey: Vec<(isize, u64)> = shadow.iter()
                                                        .filter(|(v, _)| {
                                                            let p = block.virt_to_phys(**v);
                                                            p >= lo && p <= hi
                                                        })
                                                        .map(|(v, val)| (*v, *val))
                                                        .collect();
                // Two-pass rekey: remove ALL shifted entries first, then insert
                // ALL rekeyed entries. A single-pass remove-then-insert can land
                // a rekeyed virt (`v + addr_delta`) on a not-yet-removed old virt
                // (HashMap overwrite), then the next `remove` grabs the wrong
                // value and an element is silently lost. The shift applies a
                // uniform `addr_delta`, so all new keys are distinct — the
                // insert pass is collision-free.
                for (v, _) in &to_rekey {
                    shadow.remove(v);
                }
                for (v, val) in to_rekey {
                    shadow.insert(v + addr_delta, val);
                }
            }
        }
    }

    /// Reverse stability check: every `Some` block slot must have a shadow
    /// entry — no "ghosts" (elements present in the block but lost from the
    /// shadow by a buggy remap). The forward check (`block.get(virt) ==
    /// shadow[virt]`) only verifies `shadow ⊆ block`; this verifies
    /// `block ⊆ shadow`, catching the class of bug where `apply_remove_delta`
    /// drops an entry (the lost element stays in the block, invisible forward).
    fn assert_no_ghosts<PTR: SignedBlockIndex, const OP: bool>(shadow: &std::collections::HashMap<isize, u64>,
                                                               block: &Block<u64, PTR, OP>,
                                                               step: usize) {
        for p in 0..block.buf.len() {
            if let Some(Some(val)) = block.buf.get(p) {
                let virt = block.phys_to_virt(p);
                assert_eq!(
                           shadow.get(&virt),
                           Some(val),
                           "ghost at step {step}: block phys {p} (virt {virt}) = {val} \
                     has no shadow entry (remap lost an element)",
                );
            }
        }
    }

    /// GP1 property test — the address-stability goalpost. Runs a random
    /// workload of `push_back`/`push_front`/`try_insert_before`/`remove` against
    /// a shadow `HashMap<isize, u64>` and periodically verifies that every live
    /// address in the shadow still resolves to its recorded value via
    /// `block.get`. If any mutation corrupted addresses, this fails.
    /// Random insert/remove/push workload against a shadow `virt -> value` map,
    /// periodically verifying the address-stability invariant (every live shadow
    /// address still resolves to its recorded value). Parameterized so the same
    /// workload runs against both the dense config and a spread config.
    fn run_stability_workload(addr_shift: u32, none_mask: u32, virt_offset: isize, seed: u64) {
        use std::collections::HashMap;

        let mut rng = lcg(seed);
        let mut block: Block<u64, i16> =
            Block::test_new(VecDeque::new(), addr_shift, none_mask, virt_offset);
        let mut shadow: HashMap<isize, u64> = HashMap::new();

        const STEPS: usize = 5000;
        const VERIFY_EVERY: usize = 100;
        let addr_min = i16::MIN as isize;
        let addr_max = i16::MAX as isize;

        for step in 0..STEPS {
            let op = rng() % 4;
            match op {
                0 => {
                    // push_back
                    let value = rng();
                    if let Ok(virt) = block.push_back(value) {
                        assert!(
                                (addr_min..=addr_max).contains(&virt),
                                "push_back returned out-of-range addr {virt} at step {step} \
                             (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",
                        );
                        shadow.insert(virt, value);
                    }
                }
                1 => {
                    // push_front — exercises the `1 << addr_shift` v_offset bump
                    // stability for nonzero shift.
                    let value = rng();
                    if let Ok(virt) = block.push_front(value) {
                        assert!(
                                (addr_min..=addr_max).contains(&virt),
                                "push_front returned out-of-range addr {virt} at step {step} \
                             (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",
                        );
                        shadow.insert(virt, value);
                    }
                }
                2 => {
                    // try_insert_before with anchor derived from a random live virt —
                    // exercises find_slot walking the eligible AP under none_mask.
                    if !shadow.is_empty() {
                        let live: Vec<isize> = shadow.keys().copied().collect();
                        let anchor_virt = live[(rng() as usize) % live.len()];
                        let anchor_phys = block.virt_to_phys(anchor_virt);
                        let value = rng();
                        if let Ok(InsertDelta::Free { new_virt }) =
                            block.try_insert_before(anchor_phys, value)
                        {
                            assert!(
                                    (addr_min..=addr_max).contains(&new_virt),
                                    "try_insert_before returned out-of-range addr {new_virt} at step {step} \
                                 (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",
                            );
                            shadow.insert(new_virt, value);
                        }
                    }
                }
                3 => {
                    // remove a random live virt — value must match the shadow.
                    if !shadow.is_empty() {
                        let live: Vec<isize> = shadow.keys().copied().collect();
                        let virt = live[(rng() as usize) % live.len()];
                        let expected = shadow[&virt];
                        let (got, delta) = block.remove(virt);
                        assert_eq!(
                                   got, expected,
                                   "remove returned wrong value at step {step} virt {virt} \
                             (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",
                        );
                        shadow.remove(&virt);
                        apply_remove_delta(&mut shadow, &block, virt, &delta);
                    }
                }
                _ => unreachable!(),
            }

            // Periodic (and final) address-stability check: every live shadow
            // address must still resolve to its recorded value.
            if step % VERIFY_EVERY == 0 || step == STEPS - 1 {
                for (&virt, &value) in &shadow {
                    assert_eq!(
                               block.get(virt),
                               &value,
                               "address stability broken at step {step} virt {virt}: \
                         expected {value}, got {} \
                         (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",
                               block.get(virt),
                    );
                }
                assert_no_ghosts(&shadow, &block, step);
            }
        }

        // Final full sweep (redundant with the last periodic check but makes the
        // goalpost explicit).
        for (&virt, &value) in &shadow {
            assert_eq!(block.get(virt),
                       &value,
                       "final sweep: virt {virt} (shift={addr_shift} none_mask={none_mask} v_off={virt_offset})",);
        }
    }

    #[test]
    fn gp1_address_stability() {
        // Dense config: addr_shift=0, none_mask=0, virt_offset=0 — every slot
        // eligible, push_front legal, address-stable-for-all-ops baseline.
        run_stability_workload(0, 0, 0, 0xb10c_4dd5_5555_dead);
    }

    #[test]
    fn gp1_address_stability_spread() {
        // Spread config: addr_shift=1 (translation drops the low bit),
        // none_mask=1 (stride 2 — half the slots are None-eligible gaps, so
        // find_slot walks a real AP), virt_offset=100 (nonzero, exercises
        // push_front's `1 << addr_shift` bump stability for nonzero shift and
        // try_insert into a shifted block). This is the config the trivial test
        // misses and the one that catches translation/spread-eligibility bugs.
        run_stability_workload(1, 1, 100, 0x5dead_5bad_c0de);
    }

    /// Overprovisioning contract (GP2 Step 0): a `Block<_, i32, true>` derives
    /// the SAME address space as a tight `Block<_, i16, false>` (MAX = 1<<16 =
    /// 65536 → `[-32768, 32767]`), but stores it in `i32` — 2× pointer width —
    /// so `i32::max()` strictly exceeds `addr_max`, proving the headroom that
    /// makes exhaustion impossible. Locks the derived address model: `MAX`
    /// comes from `PTR` + `OVERP`, not from an independent const.
    #[test]
    fn overprovisioned_block_keeps_address_space_with_headroom() {
        assert_eq!(<Block<u64, i32, true>>::addr_range(),
                   (-32768, 32767),
                   "overprovisioned i32 must derive the same address space as tight i16",);
        assert!(<i32 as SignedBlockIndex>::max().as_isize() > 32767,
                "i32::max ({}) must exceed addr_max (32767) — the overprovisioning headroom",
                <i32 as SignedBlockIndex>::max().as_isize(),);
        // Sanity: half_ptr tracks MAX (not PTR) — both tight i16 and
        // overprovisioned i32 give sqrt(65536) = 256.
        assert_eq!(Block::<u64, i32, true>::half_ptr(), 256);

        // Behavioral confirmation the overprovisioned block hands out a valid
        // (in-range) address. NOTE: overprovisioning does NOT give a single
        // block more capacity — addr_range is the same as tight i16, so plain
        // push_back exhausts at the same point. The headroom lives in the PTR
        // storage and only pays off during spread/readdress (Step 4), where
        // doubling the stride can push tight-PTR addresses out of range but
        // leaves the overprovisioned PTR room. The exhaustion-prevention
        // behavioral test therefore belongs in Step 4 (post-spread), not here.
        let mut block: Block<u64, i32, true> = Block::from_raw_parts(VecDeque::new(), 0, 0, 0);
        let virt = block.push_back(42u64).expect("overprovisioned block accepts a push");
        assert!((-32768..=32767).contains(&virt),
                "overprovisioned push_back must return an in-range address, got {virt}",);
    }

    /// A *tight* config whose `PTR::bit_width() >= usize::BITS` (e.g. `i64` on either
    /// 32- or 64-bit targets) collapses `MAX` to 1 → inverted address range
    /// `(0, -1)` → a silently-broken block where every insert would fail
    /// `AddressExhaustion`. `assert_capacity` must catch this at construction
    /// with a hard assert (not debug_assert), so the footgun is loud.
    #[test]
    #[should_panic(expected = "degenerate Block config")]
    fn tight_wide_ptr_is_rejected_at_construction() {
        let _ = Block::<u64, i64, false>::from_raw_parts(VecDeque::new(), 0, 0, 0);
    }

    // -----------------------------------------------------------------
    // Strategy initializer behavioral tests (GP2 Step 1).
    //
    // These are self-validating: they exercise the sign convention
    // (`virt_offset = -front_virt`) by checking that addresses grow in the
    // right direction and stay in range. A sign error in any initializer's
    // `front_virt`/`virt_offset` derivation would fail these tests.
    // -----------------------------------------------------------------

    /// Generic stability-workload runner: takes a pre-constructed block (from
    /// any strategy initializer) and runs the mixed-op loop (push_back /
    /// push_front / try_insert_before / remove) against a shadow
    /// `HashMap<isize, u64>`, periodically verifying the address-stability
    /// invariant. This is the strongest validator for sign-convention bugs —
    /// a wrong `virt_offset` direction would surface as out-of-range
    /// addresses or corrupted lookups within the first few hundred steps.
    fn run_stability_workload_on<PTR: SignedBlockIndex, const OP: bool>(block: &mut Block<u64,
                                                                                   PTR,
                                                                                   OP>,
                                                                        seed: u64) {
        use std::collections::HashMap;

        let mut rng = lcg(seed);
        let mut shadow: HashMap<isize, u64> = HashMap::new();

        const STEPS: usize = 5000;
        const VERIFY_EVERY: usize = 100;
        let (addr_min, addr_max) = Block::<u64, PTR, OP>::addr_range();

        for step in 0..STEPS {
            let op = rng() % 4;
            match op {
                0 => {
                    let value = rng();
                    if let Ok(virt) = block.push_back(value) {
                        assert!((addr_min..=addr_max).contains(&virt),
                                "push_back out-of-range {virt} at step {step}",);
                        shadow.insert(virt, value);
                    }
                }
                1 => {
                    let value = rng();
                    if let Ok(virt) = block.push_front(value) {
                        assert!((addr_min..=addr_max).contains(&virt),
                                "push_front out-of-range {virt} at step {step}",);
                        shadow.insert(virt, value);
                    }
                }
                2 => {
                    if !shadow.is_empty() {
                        let live: Vec<isize> = shadow.keys().copied().collect();
                        let anchor_virt = live[(rng() as usize) % live.len()];
                        let anchor_phys = block.virt_to_phys(anchor_virt);
                        let value = rng();
                        if let Ok(InsertDelta::Free { new_virt }) =
                            block.try_insert_before(anchor_phys, value)
                        {
                            assert!((addr_min..=addr_max).contains(&new_virt),
                                    "try_insert_before out-of-range {new_virt} at step {step}",);
                            shadow.insert(new_virt, value);
                        }
                    }
                }
                3 => {
                    if !shadow.is_empty() {
                        let live: Vec<isize> = shadow.keys().copied().collect();
                        let virt = live[(rng() as usize) % live.len()];
                        let expected = shadow[&virt];
                        let (got, delta) = block.remove(virt);
                        assert_eq!(got, expected, "remove wrong value at step {step} virt {virt}",);
                        shadow.remove(&virt);
                        apply_remove_delta(&mut shadow, &block, virt, &delta);
                    }
                }
                _ => unreachable!(),
            }

            if step % VERIFY_EVERY == 0 || step == STEPS - 1 {
                for (&virt, &value) in &shadow {
                    assert_eq!(block.get(virt),
                               &value,
                               "stability broken at step {step} virt {virt}",);
                }
                assert_no_ghosts(&shadow, &block, step);
            }
        }

        for (&virt, &value) in &shadow {
            assert_eq!(block.get(virt), &value, "final sweep: virt {virt}",);
        }
    }

    /// Append initializer: `new_append(cap)` puts the front low (`addr_min +
    /// half_ptr`), so push_back grows UP from there and push_front has
    /// `half_ptr` addresses of room below. Verifies the sign convention:
    /// `v_offset = -(addr_min + half_ptr)` (positive, since addr_min is
    /// negative) makes `phys_to_virt(0) = -v_offset = addr_min + half_ptr` —
    /// the low-but-not-minimal front address.
    #[test]
    fn new_append_behavioral() {
        let (addr_min, addr_max) = Block::<u64, i16>::addr_range();
        let half = Block::<u64, i16>::half_ptr();

        let mut block = Block::<u64, i16>::new_append(16);
        // push_back N values — addresses strictly increase, all <= addr_max.
        const N: usize = 32;
        let mut prev = isize::MIN;
        let mut first = None;
        for i in 0..N {
            let virt = block.push_back(i as u64).expect("append push_back succeeds");
            assert!(virt > prev,
                    "append push_back not strictly increasing: {virt} <= {prev} at i={i}",);
            assert!(virt <= addr_max, "append push_back {virt} > addr_max {addr_max}");
            if first.is_none() {
                first = Some(virt);
            }
            prev = virt;
        }
        // First push_back near addr_min + half_ptr (the front_virt).
        let expected_front = addr_min + half as isize;
        let first = first.unwrap();
        assert_eq!(first, expected_front,
                   "append first push_back {first} != front_virt {expected_front}",);

        // push_front half_ptr times — all succeed (half_ptr room below) and
        // stay >= addr_min.
        for i in 0..half {
            let virt = block.push_front(i as u64)
                            .unwrap_or_else(|e| panic!("append push_front {i} failed: {e:?}"));
            assert!(virt >= addr_min, "append push_front {virt} < addr_min {addr_min} at i={i}",);
        }
    }

    /// Prepend initializer: `new_prepend(cap)` puts the front high (`addr_max -
    /// half_ptr`), so push_front grows DOWN from there and push_back has
    /// `half_ptr` addresses of room above. Mirror of `new_append`. The first
    /// push_front address is `front_virt - 1` (push_front bumps v_offset by
    /// `1 << addr_shift = 1` before computing the address), so it's one below
    /// `addr_max - half_ptr`.
    #[test]
    fn new_prepend_behavioral() {
        let (addr_min, addr_max) = Block::<u64, i16>::addr_range();
        let half = Block::<u64, i16>::half_ptr();

        let mut block = Block::<u64, i16>::new_prepend(16);
        // push_front N values — addresses strictly decrease, all >= addr_min.
        const N: usize = 32;
        let mut prev = isize::MAX;
        let mut first = None;
        for i in 0..N {
            let virt = block.push_front(i as u64).expect("prepend push_front succeeds");
            assert!(virt < prev,
                    "prepend push_front not strictly decreasing: {virt} >= {prev} at i={i}",);
            assert!(virt >= addr_min, "prepend push_front {virt} < addr_min {addr_min}");
            if first.is_none() {
                first = Some(virt);
            }
            prev = virt;
        }
        // First push_front is deterministically `front_virt - 1`: `push_front`
        // bumps `v_offset` by `1 << addr_shift` (= 1 here) before returning the
        // new front address, so the first pushed element lands one address
        // below the initial `front_virt = addr_max - half_ptr`. Exact (no ±1
        // tolerance — a sign regression would miss by far more than 1).
        let expected_first = addr_max - half as isize - 1;
        let first = first.unwrap();
        assert_eq!(first, expected_first,
                   "prepend first push_front {first} != front_virt-1={expected_first}",);

        // push_back half_ptr times — all succeed (half_ptr room above) and
        // stay <= addr_max.
        for i in 0..half {
            let virt = block.push_back(i as u64)
                            .unwrap_or_else(|e| panic!("prepend push_back {i} failed: {e:?}"));
            assert!(virt <= addr_max, "prepend push_back {virt} > addr_max {addr_max} at i={i}",);
        }
    }

    /// Pluripotent initializer: `new_pluripotent(cap)` centers the `half_ptr`-
    /// wide window in the address range, so the first push_back lands near 0
    /// (center of the range). Both push_back and push_front succeed for a
    /// while. With cap=1, addr_shift=8: the single slot is at front_virt=-128,
    /// which is near 0 relative to the full [-32768, 32767] range.
    #[test]
    fn new_pluripotent_behavioral() {
        let (addr_min, addr_max) = Block::<u64, i16>::addr_range();
        let half = Block::<u64, i16>::half_ptr();

        // cap=1: one slot at front_virt = -128 (centered window [-128, 128)).
        let mut block = Block::<u64, i16>::new_pluripotent(1);
        let v0 = block.push_back(0u64).expect("pluripotent push_back");
        // |v0| < half_ptr: window is centered around 0.
        assert!(v0.abs() < half as isize,
                "pluripotent cap=1 first push_back {v0} not near 0 (|addr| < {half})",);
        assert!((addr_min..=addr_max).contains(&v0));

        // push_front also succeeds — balanced growth.
        let f0 = block.push_front(1u64).expect("pluripotent push_front");
        assert!((addr_min..=addr_max).contains(&f0));
    }

    /// Pluripotent with cap=16: same centered window, but addr_shift=4 (16
    /// slots spanning half_ptr=256 addresses). Exercise both directions.
    #[test]
    fn new_pluripotent_cap16_behavioral() {
        let (addr_min, addr_max) = Block::<u64, i16>::addr_range();
        let half = Block::<u64, i16>::half_ptr();

        let mut block = Block::<u64, i16>::new_pluripotent(16);
        // First push_back near 0 (window centered).
        let v0 = block.push_back(0u64).expect("pluripotent cap=16 push_back");
        assert!(v0.abs() < half as isize, "pluripotent cap=16 first push_back {v0} not near 0",);

        // Both directions succeed for a while. push_back yields strictly
        // increasing addresses (grows up); push_front strictly decreasing
        // (grows down) — direction asserts catch a sign error that an in-range
        // check alone would miss.
        let mut prev_back = v0;
        for i in 0..16 {
            let v = block.push_back(i as u64);
            assert!(v.is_ok(), "pluripotent push_back {i} failed");
            let v = v.unwrap();
            assert!((addr_min..=addr_max).contains(&v));
            assert!(v > prev_back, "pluripotent push_back not increasing: {v} <= {prev_back}");
            prev_back = v;
        }
        let mut prev_front = v0;
        for i in 0..16 {
            let v = block.push_front(i as u64);
            assert!(v.is_ok(), "pluripotent push_front {i} failed");
            let v = v.unwrap();
            assert!((addr_min..=addr_max).contains(&v));
            assert!(v < prev_front, "pluripotent push_front not decreasing: {v} >= {prev_front}");
            prev_front = v;
        }
    }

    /// Random initializer: `new_random(cap)` spans the full address range;
    /// front at `addr_min` (no centering slack). First push_back at addr_min,
    /// subsequent addresses increase by `2^addr_shift`. With cap=16,
    /// addr_shift=12: the block has exactly 16 addressable slots (phys 0..15)
    /// spanning [addr_min, addr_max].
    #[test]
    fn new_random_behavioral() {
        let (addr_min, addr_max) = Block::<u64, i16>::addr_range();

        let mut block = Block::<u64, i16>::new_random(16);
        // push_back N values — strictly increasing, all in range. First push
        // at addr_min (front of the full range). cap=16 → 16 addressable slots.
        const N: usize = 16;
        let mut prev = isize::MIN;
        for i in 0..N {
            let virt = block.push_back(i as u64)
                            .unwrap_or_else(|e| panic!("random push_back {i} failed: {e:?}"));
            assert!(virt > prev,
                    "random push_back not strictly increasing: {virt} <= {prev} at i={i}",);
            assert!((addr_min..=addr_max).contains(&virt), "random push_back {virt} out of range",);
            if i == 0 {
                assert_eq!(virt, addr_min, "random first push_back {virt} != addr_min {addr_min}",);
            }
            prev = virt;
        }

        // The 17th push_back (phys 16) exceeds addr_max — address exhaustion.
        assert_eq!(block.push_back(99u64),
                   Err(NotFound::AddressExhaustion),
                   "random cap=16: 17th push_back must exhaust (phys 16 → addr 32768 > addr_max)",);
    }

    /// Stability workload for each strategy initializer (tight i16).
    #[test]
    fn gp2_append_stability() {
        let mut block = Block::<u64, i16>::new_append(16);
        run_stability_workload_on(&mut block, 0xa664dd51234abcd);
    }

    #[test]
    fn gp2_prepend_stability() {
        let mut block = Block::<u64, i16>::new_prepend(16);
        run_stability_workload_on(&mut block, 0x66e64dd5678ef01);
    }

    #[test]
    fn gp2_pluripotent_stability() {
        let mut block = Block::<u64, i16>::new_pluripotent(16);
        run_stability_workload_on(&mut block, 0x9a64dd590abcdef);
    }

    #[test]
    fn gp2_random_stability() {
        let mut block = Block::<u64, i16>::new_random(16);
        run_stability_workload_on(&mut block, 0x8a64dd5feedface);
    }

    // -----------------------------------------------------------------
    // GP2 Step 3b targeted tests: gap-insertion + removal shift-to-AP.
    // -----------------------------------------------------------------

    /// Gap-insertion (Append, stride-16 AP): when the next back slot is an AP
    /// (aligned) position, the Append arm pushes a `None` vacancy there first so
    /// values never occupy aligned slots on the append path. Deterministic
    /// 2-push case: the 2nd push's next slot (phys 1) is an AP slot → a `None`
    /// gap lands at phys 1, the value at phys 2. (Later pushes reuse the gap via
    /// `Found::At` — the intended "reuse on-AP None" mechanism — so this is
    /// observed on the append path before reuse kicks in.)
    #[test]
    fn append_gap_insertion_inserts_none_at_ap_slot() {
        let mut block: Block<u64, i16> = Block::new_append(64);
        let none_mask = block.none_mask as usize;
        let v_off_phys = (block.virt_offset as usize) >> block.addr_shift;
        // `try_insert_after(0, _)` routes to `Found::Append` while the block is
        // near-empty (the back is the nearest viable end; no on-AP `None` yet).
        block.try_insert_after(0, 100).unwrap(); // next=0 (residue 0, non-AP) → phys 0
        block.try_insert_after(0, 200).unwrap(); // next=1 (residue 1, AP) → gap + phys 2
        assert_eq!(block.buf[0], Some(100), "first value at phys 0 (non-AP)");
        assert_eq!((1 + v_off_phys) & none_mask, 1, "phys 1 is an AP (residue-1) slot",);
        assert!(block.buf[1].is_none(), "phys 1 (AP) is the gap-inserted vacancy");
        assert_eq!(block.buf[2], Some(200), "value placed after the gap at phys 2");
        assert_ne!((2 + v_off_phys) & none_mask, 1, "phys 2 is non-AP (value not on an AP slot)",);
    }

    /// Removing an unaligned slot shifts the resulting `None` to the nearest
    /// aligned (AP) slot, returns `InsertDelta::Move` with the right remap, and
    /// the shifted element's address changes by `addr_delta`.
    #[test]
    fn removal_shift_moves_none_to_aligned_slot() {
        // stride 4 (none_mask 3), v_offset 0 → AP slots at phys 1,5 (residue 1).
        let buf: VecDeque<Option<u64>> = (0..8u64).map(Some).collect();
        let mut block: Block<u64, i16> = Block::from_raw_parts(buf, 0, 3, 0).with_budget(8);
        // Remove phys 2 (residue 2, unaligned). Flanking AP: phys 1 (dist 1,
        // nearer) and phys 5 (dist 3); both Some → shift left toward phys 1.
        let (got, delta) = block.remove(2);
        assert_eq!(got, 2, "removed value is old phys 2");
        match delta {
            InsertDelta::Move { new_virt, amount, minus, addr_delta } => {
                assert_eq!(new_virt, 1, "None moved to aligned phys 1 (virt 1)");
                assert_eq!(amount, 1, "one element shifted");
                assert_eq!(minus, 1, "elements shifted right (phys +1)");
                assert_eq!(addr_delta, 1, "addr_delta = 1 << addr_shift(0)");
            }
            InsertDelta::Free { .. } => panic!("expected Move, got Free"),
            InsertDelta::BlockSplit { .. } => panic!("expected Move, got BlockSplit"),
        }
        assert!(block.buf[1].is_none(), "phys 1 (AP) is now the vacancy");
        assert_eq!(block.buf[2], Some(1), "old phys 1 shifted right to phys 2");
        // The shifted element's address changed by addr_delta: old virt 1 → 2.
        // (virt 1 is now the vacancy — check via the buffer, not `get`, which
        // panics on the unoccupied slot.)
        assert!(block.buf[1].is_none(), "old virt 1 (phys 1) now None");
        assert_eq!(block.get(2), &1, "shifted element now at virt 2");
    }

    /// Removing an unaligned slot is a NO-OP (no shift) when both flanking AP
    /// slots are already `None` — the unaligned `None` is bracketed by aligned
    /// vacancies and tolerated.
    #[test]
    fn removal_shift_noop_when_flanking_ap_both_none() {
        let mut buf: VecDeque<Option<u64>> = (0..8u64).map(Some).collect();
        buf[1] = None; // AP slot 1 (residue 1) already None
        buf[5] = None; // AP slot 5 (residue 1) already None
        let mut block: Block<u64, i16> = Block::from_raw_parts(buf, 0, 3, 0).with_budget(8);
        let (got, delta) = block.remove(2); // phys 2 unaligned, flanking AP both None
        assert_eq!(got, 2);
        assert!(matches!(delta, InsertDelta::Free { .. }),
                "no-op (Free) when both flanking AP are None, got {delta:?}",);
        assert!(block.buf[2].is_none(), "phys 2 is the vacancy (no shift)");
        assert!(block.buf[1].is_none() && block.buf[5].is_none(), "flanking AP unchanged");
    }

    /// Removing a slot that's already on the AP → no shift (`Free`).
    #[test]
    fn removal_aligned_slot_is_noop() {
        let buf: VecDeque<Option<u64>> = (0..8u64).map(Some).collect();
        let mut block: Block<u64, i16> = Block::from_raw_parts(buf, 0, 3, 0).with_budget(8);
        let (got, delta) = block.remove(1); // phys 1 is AP (residue 1)
        assert_eq!(got, 1);
        assert!(matches!(delta, InsertDelta::Free { .. }), "aligned remove is no-op");
        assert!(block.buf[1].is_none());
    }

    /// Overprovisioned append: `Block::<u64, i32, true>::new_append(8)` must
    /// derive the same address space as tight i16 (MAX-keyed formulas), but
    /// store in the wider `i32`. The behavioral test proves the MAX-keyed
    /// formulas work under overprovisioning — where PTR-keyed formulas
    /// (`PTR::bit_width()`, `PTR::MAX()`) would break (they'd use `log2(i32::MAX)`
    /// instead of `log2(MAX_magnitude) = log2(half_ptr of i32)`, producing wrong
    /// addr_shift and front_virt).
    #[test]
    fn overprovisioned_append_behavioral() {
        // Same address space as tight i16 — the MAX-keyed formulas guarantee it.
        assert_eq!(<Block<u64, i32, true>>::addr_range(),
                   (-32768, 32767),
                   "overprovisioned i32 addr_range must match tight i16",);
        let half = Block::<u64, i32, true>::half_ptr();
        assert_eq!(half, 256, "overprovisioned i32 half_ptr = sqrt(MAX) = 256");

        let (addr_min, addr_max) = Block::<u64, i32, true>::addr_range();

        let mut block = Block::<u64, i32, true>::new_append(8);
        // First push_back at addr_min + half_ptr (same as tight i16).
        let v0 = block.push_back(0u64).expect("overprovisioned append push_back");
        assert_eq!(v0,
                   addr_min + half as isize,
                   "overprovisioned append first push_back {v0} != addr_min+half_ptr",);

        // push_back N values — strictly increasing, in range.
        const N: usize = 32;
        let mut prev = isize::MIN;
        for i in 0..N {
            let virt =
                block.push_back(i as u64)
                     .unwrap_or_else(|e| panic!("overprovisioned append push_back {i}: {e:?}"));
            assert!(virt > prev, "not strictly increasing at i={i}");
            assert!((addr_min..=addr_max).contains(&virt),
                    "overprovisioned push_back {virt} out of range",);
            prev = virt;
        }

        // push_front half_ptr times — all succeed, stay >= addr_min.
        for i in 0..half {
            let virt =
                block.push_front(i as u64)
                     .unwrap_or_else(|e| panic!("overprovisioned append push_front {i}: {e:?}"));
            assert!(virt >= addr_min, "overprovisioned push_front {virt} < addr_min {addr_min}",);
        }
    }

    /// Overprovisioned append stability workload.
    #[test]
    fn gp2_overprovisioned_append_stability() {
        let mut block = Block::<u64, i32, true>::new_append(8);
        run_stability_workload_on(&mut block, 0x6a64dd5cafebabe);
    }

    /// Overprovisioned pluripotent: MAX-keyed formulas must give the same
    /// `addr_shift`/`half_ptr`/centered window as tight i16 (half_ptr=256,
    /// cap=1 → addr_shift=8, front near 0). Pluripotent/random were untested
    /// under overprovisioning — only append was. This closes that gap.
    #[test]
    fn overprovisioned_pluripotent_behavioral() {
        assert_eq!(<Block<u64, i32, true>>::half_ptr(), 256);
        let (addr_min, addr_max) = <Block<u64, i32, true>>::addr_range();
        let mut block = Block::<u64, i32, true>::new_pluripotent(1);
        let v0 = block.push_back(0u64).expect("overprovisioned pluripotent push_back");
        assert!(v0.abs() < 256, "overprovisioned pluripotent first push_back {v0} not near 0",);
        assert!((addr_min..=addr_max).contains(&v0));
    }

    /// Overprovisioned random: `addr_shift = log2(MAX) - log2(cap)` must key
    /// off MAX (= 1<<16 for overprovisioned i32), NOT `PTR::bit_width()` (= 32).
    #[test]
    fn overprovisioned_random_behavioral() {
        let (addr_min, addr_max) = <Block<u64, i32, true>>::addr_range();
        let mut block = Block::<u64, i32, true>::new_random(16);
        let v0 = block.push_back(0u64).expect("overprovisioned random push_back");
        assert_eq!(v0, addr_min, "overprovisioned random first push_back {v0} != addr_min");
        let v1 = block.push_back(1u64).expect("overprovisioned random push_back 1");
        assert_eq!(v1, addr_min + (1 << 12), "overprovisioned random stride wrong: {v1}");
        assert!((addr_min..=addr_max).contains(&v1));
    }

    #[test]
    fn gp2_overprovisioned_pluripotent_stability() {
        let mut block = Block::<u64, i32, true>::new_pluripotent(16);
        run_stability_workload_on(&mut block, 0x0bad_5eed_dead);
    }

    #[test]
    fn gp2_overprovisioned_random_stability() {
        let mut block = Block::<u64, i32, true>::new_random(16);
        run_stability_workload_on(&mut block, 0x0bad_5eed_dead + 1);
    }
}
