//! `find_slot` — locate the nearest reusable `None` slot (or an end to push
//! onto) for an insert-before at physical index `anchor`.
//!
//! # Coordinate model
//!
//! Physical buffer `buf: &[Option<T>]`, indices `0..len`. Virtual addresses map
//! via `phys = (virt + v_offset) >> addr_shift`. Spread leaves `None` gaps on a
//! stride pattern in the *spread-frame* coordinate `S = phys + v_off_phys`,
//! where `v_off_phys = v_offset >> addr_shift` (the prepend-induced frame shift
//! expressed in physical units). A slot is **None-eligible** iff
//! `S & none_mask == 1` (residue 1; `none_mask = stride - 1`, a power-of-2
//! minus one). Eligible physical indices form a single arithmetic progression
//! `pos(j) = first_right + j·stride` (`…, first_right-2s, first_right-s,
//! first_right, first_right+s, first_right+2s, …`), so `first_left =
//! first_right - stride`. Masking the raw virtual address is wrong for
//! `addr_shift > 0` because the shift corrupts the low bits; `S` keeps the
//! residue clean.
//!
//! `find_slot` walks that AP outward from `anchor` in increasing `|dist|`,
//! ties → right (the `+j` side before the `-j` side at equal distance, matching
//! insert-before). Reaching a physical end whose push address is representable
//! yields `Prepend`/`Append`; exhausting the per-direction stride budget yields
//! `NotFound`. Address-exhaustion (a nonviable end hit while the other
//! direction also came up empty) is a subcase of `NotFound`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// Physical index of a `None` slot to reuse. Elements between `anchor` and
    /// `phys` shift to make room.
    At(usize),
    /// Front reached; a `push_front` address is representable.
    Prepend,
    /// Back reached; a `push_back` address is representable.
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFound {
    /// Both directions spent their stride budget without reaching an end or a
    /// `None` slot.
    OutOfBudget,
    /// At least one direction hit a nonviable end (address not representable)
    /// while the other found nothing.
    AddressExhaustion,
}

/// Knobs for `find_slot`.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// `stride - 1`, a power-of-2 minus one. `stride = none_mask + 1`.
    pub none_mask: usize,
    /// `v_offset >> addr_shift` — frame shift in physical units.
    pub v_off_phys: usize,
    /// Stride-steps scanned per direction before giving up.
    pub budget: usize,
    /// `addr_shift` from the block's address translation.
    pub addr_shift: u32,
    /// Full virtual offset (count of prepends), for end-viability checks.
    pub v_offset: usize,
    /// Representable virtual-address range, e.g. `i16::MIN..=i16::MAX`.
    pub addr_min: isize,
    pub addr_max: isize,
}

impl Params {
    #[inline]
    pub fn stride(&self) -> usize {
        self.none_mask + 1
    }
    /// Virtual address of physical slot `phys`.
    #[inline]
    fn virt_of(&self, phys: usize) -> isize {
        ((phys as isize) << self.addr_shift) - self.v_offset as isize
    }
    /// Would a `push_back` (new slot at `phys == len`) hand out a representable
    /// address?
    #[inline]
    fn back_viable(&self, len: usize) -> bool {
        self.virt_of(len) <= self.addr_max
    }
    /// Would a `push_front` (new slot at `phys 0`, `v_offset` bumped) hand out
    /// a representable address? The new front address is `-(v_offset + 1)`.
    #[inline]
    fn front_viable(&self) -> bool {
        -(self.v_offset as isize + 1) >= self.addr_min
    }
}

/// Where `align` lands relative to `anchor`, and the AP geometry around it.
#[derive(Clone, Copy)]
struct Align {
    /// First eligible index `>= anchor` (the `+j` / right side of the AP).
    first_right: isize,
    /// First eligible index `< anchor` (`first_right - stride`; the `-j` / left
    /// side).
    first_left: isize,
    stride: isize,
    /// `first_right - anchor`: distance from `anchor` to the first right-side
    /// eligible slot. `0` means `anchor` itself is eligible.
    right_delta: usize,
    /// True when the right side is nearer at every rank (ties included →
    /// right), so the shared loop probes right before left.
    right_first: bool,
}

impl Align {
    /// In-range eligible-slot counts on each side, uncapped. End resolution
    /// needs the un-capped count to know whether an end was actually reached.
    #[inline]
    fn counts(self, len: isize) -> (usize, usize) {
        let count_right = if self.first_right >= len {
            0
        } else {
            ((len - 1 - self.first_right) / self.stride + 1) as usize
        };
        let count_left = if self.first_left < 0 {
            0
        } else {
            (self.first_left / self.stride + 1) as usize
        };
        (count_right, count_left)
    }
}

/// Align `anchor` to the nearest None-eligible AP positions.
#[inline]
fn align(anchor: usize, params: &Params) -> Align {
    let stride = params.stride() as isize;
    let residue = (anchor.wrapping_add(params.v_off_phys)) & params.none_mask;
    let right_delta = (1usize.wrapping_sub(residue)) & params.none_mask;
    let first_right = anchor as isize + right_delta as isize;
    Align {
        first_right,
        first_left: first_right - stride,
        stride,
        right_delta,
        right_first: right_delta * 2 <= params.stride(),
    }
}

/// Outcome of exhausting one direction's probes without finding a `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    /// Reached the physical end with a representable push address.
    Viable,
    /// Reached the physical end but the push address is not representable.
    AddrLimit,
    /// Spent the stride budget before reaching any end.
    BudgetOut,
}

/// Map the two per-direction outcomes to a final `Result`.
#[inline]
fn resolve(right: End, left: End) -> Result<Found, NotFound> {
    match (right, left) {
        (End::Viable, _) => Ok(Found::Append),
        (_, End::Viable) => Ok(Found::Prepend),
        (End::AddrLimit, _) | (_, End::AddrLimit) => Err(NotFound::AddressExhaustion),
        _ => Err(NotFound::OutOfBudget),
    }
}

/// Classify how each direction ended and combine into the final `Result`.
#[inline]
fn resolve_ends(
    count_right: usize,
    count_left: usize,
    params: &Params,
    len: usize,
) -> Result<Found, NotFound> {
    let end_right = if count_right > params.budget {
        End::BudgetOut
    } else if params.back_viable(len) {
        End::Viable
    } else {
        End::AddrLimit
    };
    let end_left = if count_left > params.budget {
        End::BudgetOut
    } else if params.front_viable() {
        End::Viable
    } else {
        End::AddrLimit
    };
    resolve(end_right, end_left)
}

/// Step `delta` from `start` for `count` eligible slots; return the first
/// `None` index, or `None` if every probe is `Some`.
#[inline]
fn scan<T>(buf: &[Option<T>], start: isize, delta: isize, count: usize) -> Option<usize> {
    let mut pos = start;
    for _ in 0..count {
        if buf[pos as usize].is_none() {
            return Some(pos as usize);
        }
        pos += delta;
    }
    None
}

/// Locate the nearest `None` slot (or a viable end) for an insert-before at
/// `anchor`.
///
/// Precomputes each side's in-range eligible-slot count and caps it at
/// `budget`, then probes outward in two phases: phase 1 walks both sides in
/// lockstep for the shared count (near side first, per `right_first`), phase 2
/// drains the longer side's remainder. When `anchor` itself is eligible
/// (`right_delta == 0`) every rank ties, so a leading solo probe of `R_0` fixes
/// tie→right before the pairs start.
pub fn find_twophase<T>(buf: &[Option<T>], anchor: usize, params: &Params) -> Result<Found, NotFound> {
    let len = buf.len();
    let len_isize = len as isize;
    let aligned = align(anchor, params);
    let first_right = aligned.first_right;
    let first_left = aligned.first_left;
    let stride = aligned.stride;

    let (count_right, count_left) = aligned.counts(len_isize);
    // Probes actually performed per side, capped at budget.
    let mut probes_right = count_right.min(params.budget);
    let probes_left = count_left.min(params.budget);

    // When `anchor` is itself eligible, R_0 sits at `anchor` and ties L_0 at
    // distance 0. Tie→right: probe R_0 alone, then pair R_1/L_1, R_2/L_2, …
    let mut pos_right = first_right;
    if aligned.right_delta == 0 {
        if buf[first_right as usize].is_none() {
            return Ok(Found::At(first_right as usize));
        }
        pos_right = first_right + stride;
        probes_right -= 1;
    }
    let mut pos_left = first_left;

    // Phase 1: both sides for the shared count, near side first. The
    // `right_first` selection is loop-invariant, so it's hoisted into one of
    // two specialized bodies rather than re-branched each iteration.
    let shared = probes_right.min(probes_left);
    if aligned.right_first {
        for _ in 0..shared {
            if buf[pos_right as usize].is_none() {
                return Ok(Found::At(pos_right as usize));
            }
            pos_right += stride;
            if buf[pos_left as usize].is_none() {
                return Ok(Found::At(pos_left as usize));
            }
            pos_left -= stride;
        }
    } else {
        for _ in 0..shared {
            if buf[pos_left as usize].is_none() {
                return Ok(Found::At(pos_left as usize));
            }
            pos_left -= stride;
            if buf[pos_right as usize].is_none() {
                return Ok(Found::At(pos_right as usize));
            }
            pos_right += stride;
        }
    }
    // Phase 2: drain whichever side still has probes left (at most one —
    // `shared` is the smaller cap, so only the larger side has a remainder; the
    // other call gets a zero count and compiles away).
    if let Some(slot) = scan(buf, pos_right, stride, probes_right - shared) {
        return Ok(Found::At(slot));
    }
    if let Some(slot) = scan(buf, pos_left, -stride, probes_left - shared) {
        return Ok(Found::At(slot));
    }

    resolve_ends(count_right, count_left, params, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_params(stride: usize) -> Params {
        Params {
            none_mask: stride - 1,
            v_off_phys: 3,
            budget: 8,
            addr_shift: 0,
            v_offset: 3,
            addr_min: i16::MIN as isize,
            addr_max: i16::MAX as isize,
        }
    }

    fn blank(len: usize) -> Vec<Option<u64>> {
        vec![Some(0); len]
    }

    /// Set `None` at the `rank`-th eligible slot on each side of `anchor`.
    fn place_none_at_rank(buf: &mut [Option<u64>], anchor: usize, params: &Params, rank: usize) {
        let aligned = align(anchor, params);
        let right_idx = (aligned.first_right + aligned.stride * rank as isize) as usize;
        if right_idx < buf.len() {
            buf[right_idx] = None;
        }
        let left_idx = (aligned.first_left - aligned.stride * rank as isize) as isize;
        if left_idx >= 0 {
            buf[left_idx as usize] = None;
        }
    }

    #[test]
    fn align_lands_on_eligible() {
        for stride in [2usize, 4, 8, 16] {
            let base = base_params(stride);
            for anchor in 0..64 {
                for v_off_phys in 0..stride {
                    let mut params = base;
                    params.v_off_phys = v_off_phys;
                    let aligned = align(anchor, &params);
                    assert_eq!(
                        ((aligned.first_right as usize + v_off_phys) & params.none_mask, 1usize),
                        (1, 1),
                        "first_right not eligible: stride={stride} anchor={anchor} v_off_phys={v_off_phys} first_right={}",
                        aligned.first_right
                    );
                    if aligned.first_left >= 0 {
                        assert_eq!(
                            ((aligned.first_left as usize + v_off_phys) & params.none_mask, 1usize),
                            (1, 1),
                            "first_left not eligible: stride={stride} anchor={anchor} v_off_phys={v_off_phys} first_left={}",
                            aligned.first_left
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tie_goes_right_exact() {
        // right_delta == stride/2: equidistant tie at the nearest rank.
        let stride = 4;
        let params = base_params(stride);
        let anchor = 8; // anchor&3==0 → right_delta==2
        let aligned = align(anchor, &params);
        assert_eq!(aligned.first_right - anchor as isize, 2);
        assert_eq!(anchor as isize - aligned.first_left, 2);
        let mut buf = blank(64);
        buf[aligned.first_right as usize] = None;
        buf[aligned.first_left as usize] = None;
        assert_eq!(
            find_twophase(&buf, anchor, &params),
            Ok(Found::At(aligned.first_right as usize))
        );
    }

    #[test]
    fn tie_goes_right_when_anchor_eligible() {
        // right_delta == 0: anchor itself eligible → every R_k/L_k pair ties;
        // tie→right must pick the +j side. This is the case the old alternation
        // got wrong.
        let stride = 8;
        let mut params = base_params(stride);
        params.v_off_phys = 1; // (anchor+1)&7==1 → anchor eligible at anchor=24
        let anchor = 24;
        let aligned = align(anchor, &params);
        assert_eq!(aligned.first_right as usize, anchor, "anchor must be eligible");
        // Nones equidistant at rank 2: first_right+2s (right) and first_right-2s
        // (left), both dist 16.
        let mut buf = blank(128);
        buf[anchor + 2 * stride] = None; // right, dist 16
        buf[anchor - 2 * stride] = None; // left, dist 16
        assert_eq!(
            find_twophase(&buf, anchor, &params),
            Ok(Found::At(anchor + 2 * stride)),
            "tie must resolve right when anchor is eligible"
        );
    }

    #[test]
    fn finds_nearest_none() {
        let stride = 4;
        let params = base_params(stride);
        let anchor = 32;
        let mut buf = blank(128);
        place_none_at_rank(&mut buf, anchor, &params, 3);
        let aligned = align(anchor, &params);
        let cand_right = aligned.first_right + 3 * aligned.stride;
        let cand_left = aligned.first_left - 3 * aligned.stride;
        let expected = Found::At(
            if (aligned.first_right - anchor as isize).abs() <= (anchor as isize - aligned.first_left).abs() {
                cand_right
            } else {
                cand_left
            } as usize,
        );
        assert_eq!(find_twophase(&buf, anchor, &params).unwrap(), expected);
    }

    #[test]
    fn append_and_prepend() {
        let stride = 4;
        let mut params = base_params(stride);
        params.budget = 8;
        let len = 4096usize;
        let buf = blank(len);
        let anchor_append = len - 8 * stride;
        let anchor_prepend = 8 * stride;
        assert_eq!(find_twophase(&buf, anchor_append, &params).unwrap(), Found::Append);
        assert_eq!(find_twophase(&buf, anchor_prepend, &params).unwrap(), Found::Prepend);
    }

    #[test]
    fn miss_is_out_of_budget() {
        let stride = 4;
        let params = base_params(stride);
        let buf = blank(4096);
        let anchor = 2048;
        assert_eq!(find_twophase(&buf, anchor, &params), Err(NotFound::OutOfBudget));
    }

    #[test]
    fn address_exhaustion() {
        let stride = 2;
        let mut params = base_params(stride);
        params.v_offset = 20;
        params.v_off_phys = 20;
        params.addr_min = -10;
        params.addr_max = 100;
        params.budget = 8;
        let buf = blank(4096);
        let anchor = 5;
        assert_eq!(
            find_twophase(&buf, anchor, &params),
            Err(NotFound::AddressExhaustion)
        );
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
    /// sides, sort by distance (tie → right), return the first `None`; else
    /// resolve ends exactly like `find_twophase`. Independent of the production
    /// strategy, so it's a real correctness oracle.
    fn find_ref(buf: &[Option<u64>], anchor: usize, params: &Params) -> Result<Found, NotFound> {
        let len = buf.len() as isize;
        let stride = params.stride() as isize;
        let aligned = align(anchor, params);
        let first_right = aligned.first_right;
        let right_delta = aligned.right_delta as isize;
        let mut candidates: Vec<(usize, isize)> = Vec::new(); // (dist, pos)
        let mut rank_right = 0isize;
        while rank_right < params.budget as isize {
            let pos = first_right + rank_right * stride;
            if pos >= len {
                break;
            }
            candidates.push(((right_delta + rank_right * stride) as usize, pos));
            rank_right += 1;
        }
        let mut rank_left = 1isize;
        while rank_left <= params.budget as isize {
            let pos = first_right - rank_left * stride;
            if pos < 0 {
                break;
            }
            candidates.push(((rank_left * stride - right_delta) as usize, pos));
            rank_left += 1;
        }
        // sort by dist asc, tie → right (greater pos first)
        candidates.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0).then_with(|| rhs.1.cmp(&lhs.1)));
        for (_, pos) in candidates {
            if buf[pos as usize].is_none() {
                return Ok(Found::At(pos as usize));
            }
        }
        let (count_right, count_left) = aligned.counts(len);
        resolve_ends(count_right, count_left, params, buf.len())
    }

    #[test]
    fn matches_reference() {
        let mut rng = lcg(0xa11d_5175_7151_5e9b);
        for _ in 0..8000 {
            let stride = 1usize << (1 + (rng() % 4));
            let budget = 1 + (rng() % 20) as usize;
            let v_off_phys = (rng() % stride as u64) as usize;
            let len = 1 + (rng() % 512) as usize;
            let anchor = (rng() % len as u64) as usize;
            let params = Params {
                none_mask: stride - 1,
                v_off_phys,
                budget,
                addr_shift: 0,
                v_offset: v_off_phys,
                addr_min: -(1 + (rng() % 50) as isize),
                addr_max: (rng() % 50) as isize + len as isize,
            };
            let mut buf = blank(len);
            let mut fill_rng = lcg(rng());
            for idx in 0..len {
                if ((idx + v_off_phys) & params.none_mask) == 1 && fill_rng() % 3 == 0 {
                    buf[idx] = None;
                }
            }
            let expected = find_ref(&buf, anchor, &params);
            assert_eq!(
                find_twophase(&buf, anchor, &params),
                expected,
                "twophase!=ref stride={stride} anchor={anchor} len={len} v_off_phys={v_off_phys} bud={budget}"
            );
        }
    }
}