#[cfg(test)]
mod tests {
    use crate::block::Block;
    use crate::find_slot::*;
    use std::collections::VecDeque;

    /// Base args — returns `(none_mask, v_off_phys, budget, addr_shift, v_offset)`.
    /// `addr_min`/`addr_max` are fixed by `PTR = i16` and read inside
    /// `Block::find_slot`, so callers no longer pass them.
    fn base_args(stride: usize) -> (usize, usize, usize, u32, usize) {
        (stride - 1, 3, 8, 0, 3)
    }

    fn blank(len: usize) -> VecDeque<Option<u64>> {
        vec![Some(0); len].into()
    }

    /// Set `None` at the `rank`-th eligible slot on each side of `anchor`.
    fn place_none_at_rank(buf: &mut VecDeque<Option<u64>>,
                          anchor: usize,
                          none_mask: usize,
                          v_off_phys: usize,
                          rank: usize) {
        // Bias doesn't affect geometry (first_right/first_left/stride) — only
        // right_first, which this helper doesn't use. Pass Right arbitrarily.
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
        let right_idx = (aligned.first_right + aligned.stride * rank as isize) as usize;
        if right_idx < buf.len() {
            buf[right_idx] = None;
        }
        let left_idx = (aligned.first_left - aligned.stride * rank as isize) as isize;
        if left_idx >= 0 {
            buf[left_idx as usize] = None;
        }
    }

    /// Build a `Block<u64, i16>` from the test args. `addr_min`/`addr_max` come
    /// from `PTR = i16` inside `find_slot`.
    /// Build a block with a specific scan budget (test convenience).
    fn make_block_budget(buf: VecDeque<Option<u64>>,
                         addr_shift: u32,
                         none_mask: usize,
                         v_offset: usize,
                         budget: usize)
                         -> Block<u64, i16> {
        Block::from_raw_parts(buf, addr_shift, none_mask as u32, v_offset as isize)
            .with_budget(budget)
    }

    #[test]
    fn align_lands_on_eligible() {
        for stride in [2usize, 4, 8, 16] {
            let (none_mask, _, _, _, _) = base_args(stride);
            for anchor in 0..64 {
                for v_off_phys in 0..stride {
                    let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
                    assert_eq!(((aligned.first_right as usize + v_off_phys) & none_mask, 1usize),
                               (1, 1),
                               "first_right not eligible: stride={stride} anchor={anchor} v_off_phys={v_off_phys} first_right={}",
                               aligned.first_right);
                    if aligned.first_left >= 0 {
                        assert_eq!(((aligned.first_left as usize + v_off_phys) & none_mask,
                                    1usize),
                                   (1, 1),
                                   "first_left not eligible: stride={stride} anchor={anchor} v_off_phys={v_off_phys} first_left={}",
                                   aligned.first_left);
                    }
                }
            }
        }
    }

    #[test]
    fn tie_goes_right_exact() {
        // right_delta == stride/2: equidistant tie at the nearest rank.
        let stride = 4;
        let (none_mask, v_off_phys, budget, addr_shift, v_offset) = base_args(stride);
        let anchor = 8; // anchor&3==0 → right_delta==2
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
        assert_eq!(aligned.first_right - anchor as isize, 2);
        assert_eq!(anchor as isize - aligned.first_left, 2);
        let mut buf = blank(64);
        buf[aligned.first_right as usize] = None;
        buf[aligned.first_left as usize] = None;
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Right),
                   Ok(Found::At(aligned.first_right as usize)));
    }

    #[test]
    fn tie_goes_left_exact() {
        // right_delta == stride/2: equidistant tie at the nearest rank, left bias.
        let stride = 4;
        let (none_mask, v_off_phys, budget, addr_shift, v_offset) = base_args(stride);
        let anchor = 8; // anchor&3==0 → right_delta==2
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Left);
        assert_eq!(aligned.first_right - anchor as isize, 2);
        assert_eq!(anchor as isize - aligned.first_left, 2);
        assert!(!aligned.right_first, "left bias at tie → right_first false");
        let mut buf = blank(64);
        buf[aligned.first_right as usize] = None;
        buf[aligned.first_left as usize] = None;
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Left),
                   Ok(Found::At(aligned.first_left as usize)),
                   "left bias tie must resolve to the left slot");
    }

    #[test]
    fn tie_goes_left_when_anchor_eligible() {
        // right_delta == 0: anchor eligible → it's skipped (excluded as a
        // candidate), and the paired loop probes left first under Left bias.
        // Place Nones at rank 2 on both sides (equidistant) — left bias must
        // pick the left one.
        let stride = 8;
        let (none_mask, _, budget, addr_shift, _) = base_args(stride);
        let v_offset = 1usize;
        let v_off_phys = v_offset >> addr_shift;
        let anchor = 24;
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Left);
        assert_eq!(aligned.first_right as usize, anchor, "anchor must be eligible");
        assert!(!aligned.right_first, "left bias anchor-eligible → right_first false");
        let mut buf = blank(128);
        buf[anchor] = Some(0); // anchor is Some → the skip past it fires
        buf[anchor + 2 * stride] = None; // right, dist 16
        buf[anchor - 2 * stride] = None; // left, dist 16
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Left),
                   Ok(Found::At(anchor - 2 * stride)),
                   "left bias tie must resolve left when anchor is eligible");
    }

    #[test]
    fn bias_does_not_change_strictly_nearest() {
        // When one side is strictly closer (not a tie), bias is irrelevant:
        // the strictly-nearer slot wins regardless of bias.
        let stride = 4;
        let (none_mask, v_off_phys, budget, addr_shift, v_offset) = base_args(stride);
        let anchor = 32;
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
        // Place None only on the right at rank 1 (dist right_delta + stride).
        let right_idx = (aligned.first_right + aligned.stride) as usize;
        let mut buf = blank(128);
        buf[right_idx] = None;
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        // Both biases must return the same strictly-nearer right slot.
        assert_eq!(block.find_slot(anchor, Bias::Right), Ok(Found::At(right_idx)));
        let mut buf2 = blank(128);
        buf2[right_idx] = None;
        let block2 = make_block_budget(buf2, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block2.find_slot(anchor, Bias::Left), Ok(Found::At(right_idx)));
    }

    #[test]
    fn both_ends_viable_follows_bias() {
        // Both ends viable, no None within budget → end resolution picks by
        // bias. This is the case the slot-differential CAN'T validate: impl +
        // oracle agreed on it whether right or wrong, so the 40k assertions
        // gave it zero correctness coverage. Here we assert the ACTUAL expected
        // value (not just impl == oracle).
        let stride = 4;
        let (none_mask, _v_off_phys, budget, addr_shift, v_offset) = base_args(stride);
        let len = 12;
        let anchor = 6; // (6 + v_off_phys=3) & 3 == 1 → eligible; first_right=6, first_left=2
        // blank = all Some → scan finds no None → falls to end resolution.
        // Both ends viable (back: 12-3=9 <= 32767; front: -(3+1)=-4 >= -32768)
        // and both sides have slots (count_right=2, count_left=1, budget=8).
        let block = make_block_budget(blank(len), addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Right),
                   Ok(Found::Append),
                   "right bias + both ends viable → Append (back)");
        let block2 = make_block_budget(blank(len), addr_shift, none_mask, v_offset, budget);
        assert_eq!(block2.find_slot(anchor, Bias::Left),
                   Ok(Found::Prepend),
                   "left bias + both ends viable → Prepend (front)");
    }

    #[test]
    fn tie_goes_right_when_anchor_eligible() {
        // right_delta == 0: anchor itself eligible → every R_k/L_k pair ties;
        // tie→right must pick the +j side. This is the case the old alternation
        // got wrong.
        let stride = 8;
        let (none_mask, _, budget, addr_shift, _) = base_args(stride);
        // v_off_phys = 1 → (anchor+1)&7==1 → anchor eligible at anchor=24.
        // With addr_shift=0, v_offset = v_off_phys = 1.
        let v_offset = 1usize;
        let v_off_phys = v_offset >> addr_shift;
        let anchor = 24;
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
        assert_eq!(aligned.first_right as usize, anchor, "anchor must be eligible");
        // Nones equidistant at rank 2: first_right+2s (right) and first_right-2s
        // (left), both dist 16.
        let mut buf = blank(128);
        buf[anchor + 2 * stride] = None; // right, dist 16
        buf[anchor - 2 * stride] = None; // left, dist 16
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Right),
                   Ok(Found::At(anchor + 2 * stride)),
                   "tie must resolve right when anchor is eligible");
    }

    #[test]
    fn finds_nearest_none() {
        let stride = 4;
        let (none_mask, v_off_phys, budget, addr_shift, v_offset) = base_args(stride);
        let anchor = 32;
        let mut buf = blank(128);
        place_none_at_rank(&mut buf, anchor, none_mask, v_off_phys, 3);
        let aligned = align(anchor, none_mask, v_off_phys, Bias::Right);
        let cand_right = aligned.first_right + 3 * aligned.stride;
        let cand_left = aligned.first_left - 3 * aligned.stride;
        let expected = Found::At(if (aligned.first_right - anchor as isize).abs()
                                    <= (anchor as isize - aligned.first_left).abs()
                                 {
                                     cand_right
                                 } else {
                                     cand_left
                                 } as usize);
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Right).unwrap(), expected);
    }

    #[test]
    fn append_and_prepend() {
        let stride = 4;
        let (none_mask, _, budget, addr_shift, v_offset) = base_args(stride);
        let len = 4096usize;
        let anchor_append = len - 8 * stride;
        let anchor_prepend = 8 * stride;
        let block = make_block_budget(blank(len), addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor_append, Bias::Right).unwrap(), Found::Append);
        assert_eq!(block.find_slot(anchor_prepend, Bias::Right).unwrap(), Found::Prepend);
    }

    #[test]
    fn miss_is_out_of_budget() {
        let stride = 4;
        let (none_mask, _, budget, addr_shift, v_offset) = base_args(stride);
        let buf = blank(4096);
        let anchor = 2048;
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        assert_eq!(block.find_slot(anchor, Bias::Right), Err(NotFound::OutOfBudget));
    }

    #[test]
    fn address_exhaustion() {
        // With `PTR = i16` the address range is fixed at `[i16::MIN, i16::MAX]`.
        // To make an end non-viable we push `virt_offset` to a near-boundary
        // value: `virt_offset = i16::MIN` makes `phys_to_virt(len) = (len <<
        // addr_shift) - i16::MIN = (len << addr_shift) + 32768 > i16::MAX` for
        // any `len >= 1` → back end non-viable. Anchor near the end so the right
        // side has 0 in-range eligible slots (`count_right == 0 <= budget` →
        // `AddrLimit`, not `BudgetOut`); the left side has many (`count_left >
        // budget` → `BudgetOut`). `resolve(AddrLimit, BudgetOut)` =
        // `AddressExhaustion`.
        let stride = 4;
        let none_mask = stride - 1;
        let budget = 8;
        let addr_shift = 0u32;
        let virt_offset = i16::MIN as isize; // -32768
        let len = 100usize;
        let buf = blank(len);
        let anchor = len - 2; // first_right = anchor + 3 = 101 >= len → count_right = 0
        let block: Block<u64, i16> = Block::from_raw_parts(buf,
                                                           addr_shift,
                                                           none_mask as u32,
                                                           virt_offset).with_budget(budget);
        assert_eq!(block.find_slot(anchor, Bias::Right), Err(NotFound::AddressExhaustion));
    }

    #[test]
    fn budget_zero_does_not_underflow() {
        // Regression: the anchor-skip path did `probes_right -= 1` without
        // checking `budget`, so `budget == 0` underflowed `probes_right` to
        // `usize::MAX` (release) and sent `scan`'s `get_unchecked` past the
        // buffer. Setup: anchor eligible (`right_delta == 0`), slot `Some`, so
        // the skip enters and takes the `-= 1` branch — but the
        // `probes_right > 0` guard now blocks it.
        let none_mask = 3usize; // stride 4
        let budget = 0usize;
        let addr_shift = 0u32;
        let v_offset = 1usize; // (anchor + 1) & 3 == 1 at anchor 0 → eligible
        let buf = blank(64); // slot 0 is Some
        let anchor = 0;
        let block = make_block_budget(buf, addr_shift, none_mask, v_offset, budget);
        // With zero probe budget no scan runs, so `resolve_ends` decides: the
        // left side has 0 in-range eligible slots (not budget-out) and
        // `push_front` is viable → `Prepend`. The assertion's real point is
        // that this returns cleanly instead of underflowing `probes_right` to
        // `usize::MAX` and UB-ing in `scan`.
        let res = block.find_slot(anchor, Bias::Right);
        assert_eq!(res, Ok(Found::Prepend), "budget=0 must not underflow");
    }

    /// Deterministic xorshift.
    fn lcg(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Brute-force reference: collect every in-budget eligible slot on both
    /// sides, sort by distance with a bias-governed tie-break, return the
    /// first `None`; else resolve ends. The SCAN (sort-then-first-None) is an
    /// independent strategy from the production two-phase probe, so it
    /// cross-checks the scan/tie-break order — but geometry (`align`), slot
    /// counts, and end resolution (`resolve_ends`) are SHARED with the impl,
    /// so this does NOT independently validate end resolution (the block.rs
    /// `oracle` does — it recomputes geometry + ends from first principles).
    /// `addr_min`/`addr_max` are always `i16::MIN`/`i16::MAX` (matching
    /// `PTR = i16`).
    fn find_ref(buf: &VecDeque<Option<u64>>,
                anchor: usize,
                none_mask: usize,
                v_off_phys: usize,
                budget: usize,
                addr_shift: u32,
                v_offset: usize,
                addr_min: isize,
                addr_max: isize,
                bias: Bias)
                -> Result<Found, NotFound> {
        let len = buf.len() as isize;
        let stride = (none_mask + 1) as isize;
        let aligned = align(anchor, none_mask, v_off_phys, bias);
        let first_right = aligned.first_right;
        let right_delta = aligned.right_delta as isize;
        let mut candidates: Vec<(usize, isize)> = Vec::new(); // (dist, pos)
        // `right_delta == 0` → anchor sits at right rank 0; excluded by
        // contract (occupied), so start right ranks at 1.
        let mut rank_right = (right_delta == 0) as isize;
        while rank_right < budget as isize {
            let pos = first_right + rank_right * stride;
            if pos >= len {
                break;
            }
            candidates.push(((right_delta + rank_right * stride) as usize, pos));
            rank_right += 1;
        }
        let mut rank_left = 1isize;
        while rank_left <= budget as isize {
            let pos = first_right - rank_left * stride;
            if pos < 0 {
                break;
            }
            candidates.push(((rank_left * stride - right_delta) as usize, pos));
            rank_left += 1;
        }
        // sort by dist asc; tie-break by bias: Right → greater pos first
        // (right side), Left → lesser pos first (left side).
        match bias {
            Bias::Right => {
                candidates.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0).then_with(|| rhs.1.cmp(&lhs.1)));
            }
            Bias::Left => {
                candidates.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0).then_with(|| lhs.1.cmp(&rhs.1)));
            }
        }
        for (_, pos) in candidates {
            if buf[pos as usize].is_none() {
                return Ok(Found::At(pos as usize));
            }
        }
        let (count_right, count_left) = aligned.counts(len);
        resolve_ends(bias,
                     count_right,
                     count_left,
                     budget,
                     addr_shift,
                     v_offset,
                     addr_min,
                     addr_max,
                     buf.len())
    }

    #[test]
    fn matches_reference() {
        let mut rng = lcg(0xa11d_5175_7151_5e9b);
        for _ in 0..8000 {
            let stride = 1usize << (1 + (rng() % 4)); // 2, 4, 8, 16
            let none_mask = stride - 1;
            let budget = 1 + (rng() % 20) as usize;
            let addr_shift = (rng() % 4) as u32; // 0..=3 (was always 0)
            let len = 1 + (rng() % 512) as usize;
            let anchor = (rng() % len as u64) as usize;

            // Derive `v_off_phys = v_offset >> addr_shift`. Normally pick
            // `v_off_phys` in `0..stride` and set `v_offset = v_off_phys <<
            // addr_shift`. Occasionally push `v_offset` to a near-i16-boundary
            // value so ends become non-viable (address-exhaustion coverage that
            // previously came from sweeping tiny `addr_min`/`addr_max`).
            let near_boundary = rng() % 8 == 0;
            let v_offset_isize;
            let v_off_phys;
            if near_boundary {
                let boundary = if rng() % 2 == 0 { i16::MAX as isize } else { i16::MIN as isize };
                v_offset_isize = boundary + (rng() % 100) as isize - 50;
                v_off_phys = (v_offset_isize as usize) >> addr_shift;
            } else {
                v_off_phys = (rng() % stride as u64) as usize;
                v_offset_isize = (v_off_phys as isize) << addr_shift;
            }
            let v_offset = v_offset_isize as usize;
            let addr_min = i16::MIN as isize;
            let addr_max = i16::MAX as isize;

            let mut buf = blank(len);
            let mut fill_rng = lcg(rng());
            for idx in 0..len {
                if ((idx + v_off_phys) & none_mask) == 1 && fill_rng() % 3 == 0 {
                    buf[idx] = None;
                }
            }
            // Test BOTH biases against a bias-aware oracle.
            for bias in [Bias::Right, Bias::Left] {
                let expected = find_ref(&buf, anchor, none_mask, v_off_phys, budget, addr_shift,
                                        v_offset, addr_min, addr_max, bias);
                let block: Block<u64, i16> =
                    Block::from_raw_parts(buf.clone(),
                                          addr_shift,
                                          none_mask as u32,
                                          v_offset_isize).with_budget(budget);
                assert_eq!(
                           block.find_slot(anchor, bias),
                           expected,
                           "find_slot!=ref stride={stride} anchor={anchor} len={len} \
                     v_off_phys={v_off_phys} bud={budget} shift={addr_shift} \
                     v_offset={v_offset_isize} bias={bias:?}"
                );
            }
        }
    }
}
