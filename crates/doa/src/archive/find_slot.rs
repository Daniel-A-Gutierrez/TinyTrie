//! Locate the nearest reusable `None` slot (or a viable end) for an insert at
//! physical index `anchor`. The `anchor` slot itself is never a candidate —
//! it's occupied by contract (the caller is inserting next to a live slot).
//!
//! Eligible slots form one arithmetic progression in the spread-frame
//! coordinate `S = phys + v_off_phys` (`v_off_phys = v_offset >> addr_shift`):
//! a slot is None-eligible iff `S & none_mask == 1` (`none_mask = stride - 1`).
//! `find_slot` walks that AP outward from `anchor` by increasing `|dist|`; a
//! [`Bias`] breaks ties (`Right` → `+j`, the default; `Left` → `-j`). Reaching
//! a physical end with a representable push address yields
//! `Prepend`/`Append`; exhausting the per-direction stride budget yields
//! `NotFound` (address-exhaustion is a subcase).

/// Tie-break direction for `find_slot`: `Right` resolves equidistant ties to
/// the `+j` side (default, matches insert-after); `Left` to the `-j` side
/// (insert-before). It also picks which side the phase-1 loop probes first.
/// A preference, not a guarantee — if only the "wrong" side has a `None`
/// within budget, that side is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    Left,
    Right,
}

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

/// Virtual address of physical slot `phys`: `(phys << addr_shift) - v_offset`.
/// End-viability use only — the scan hot path uses `v_off_phys`. `v_offset` is
/// the block's `virt_offset: isize` as `usize` (wrapping cast, round-trips).
/// The only checked precondition is `addr_shift < usize::BITS` (a too-large
/// shift would wrap-mask in release); a `debug_assert!` guards it. An
/// out-of-range *value* is a normal "not viable" result, not overflow. The
/// single copy of the formula — `phys_to_virt`/`back_viable`/`front_viable`
/// delegate here.
#[inline]
pub(crate) fn virt_of(phys: usize, addr_shift: u32, v_offset: usize) -> isize {
    // `wrapping_sub` on `usize` then `as isize` is bit-identical to
    // `(phys << addr_shift) as isize - virt_offset`, giving correct negative
    // addresses (the `push_front` case) without a sign-related panic.
    debug_assert!(phys.checked_shl(addr_shift).is_some(), "virt_of: addr_shift >= usize::BITS");
    (phys << addr_shift).wrapping_sub(v_offset) as isize
}

/// Would a `push_back` (new slot at `phys == len`) hand out a representable
/// address?
#[inline]
pub(crate) fn back_viable(len: usize, addr_shift: u32, v_offset: usize, addr_max: isize) -> bool {
    virt_of(len, addr_shift, v_offset) <= addr_max
}

/// Would a `push_front` (new slot at `phys 0`, `v_offset` bumped) hand out a
/// representable address? Bumps `v_offset` by `1 << addr_shift` (one slot's
/// worth of virt), so the new front is `-(v_offset + (1 << addr_shift))` —
/// stable for all `addr_shift`. The three preconditions (`addr_shift <
/// isize::BITS`, no `v_offset + step` overflow, operand != `isize::MIN` before
/// negation) are invariant violations guarded by `debug_assert!`; in release the
/// ops wrap. A candidate below `addr_min` is the normal "not viable" `false`,
/// not a panic.
#[inline]
pub(crate) fn front_viable(addr_shift: u32, v_offset: usize, addr_min: isize) -> bool {
    debug_assert!(1isize.checked_shl(addr_shift).is_some(),
                  "front_viable: addr_shift >= isize::BITS");
    let step = 1isize << addr_shift;
    let v_off = v_offset as isize;
    debug_assert!(v_off.checked_add(step).is_some(), "front_viable: v_offset + step overflow");
    let sum = v_off + step;
    debug_assert!(sum.checked_neg().is_some(), "front_viable: negate isize::MIN");
    (-sum) >= addr_min
}

/// AP geometry around `anchor` produced by [`align`].
#[derive(Clone, Copy)]
pub(crate) struct ScanParameters {
    /// First eligible index `>= anchor` (right side of the AP).
    pub(crate) first_right: isize,
    /// First eligible index `< anchor` (`first_right - stride`; left side).
    pub(crate) first_left:  isize,
    pub(crate) stride:      isize,
    /// `first_right - anchor`: distance to the first right-side eligible slot.
    /// `0` means `anchor` is aligned. 
    pub(crate) right_delta: usize,
    /// True when the right side is probed first in the shared loop (ties →
    /// right). `Bias::Left` makes ties go left, so this is false at ties.
    pub(crate) right_first: bool,
}

impl ScanParameters {
    /// In-range eligible-slot counts per side, uncapped. End resolution needs
    /// the uncapped count to know whether an end was actually reached.
    #[inline]
    pub(crate) fn counts(self, len: isize) -> (usize, usize) {
        let count_right = if self.first_right < len  {((len - 1 - self.first_right) / self.stride ) as usize} 
            else {0};
        let count_left = if self.first_left < 0 { 0 } 
            else { (self.first_left / self.stride ) as usize };
        (count_right, count_left)
    }

    /// Scan `anchor` to the nearest None-eligible AP positions (`stride =
    /// none_mask + 1`). `bias` picks the tie direction: `Right` uses `<=` (ties →
    /// right, the default); `Left` uses strict `<` and forces `right_first = false`
    /// when `anchor` itself is eligible (`right_delta == 0`) — see below.

    #[inline]
    pub(crate) fn new(anchor: usize, none_mask: usize, v_off_phys: usize, bias: Bias) -> ScanParameters {
        let stride = (none_mask + 1) as isize;
        let residue = (anchor.wrapping_add(v_off_phys)) & none_mask;
        let right_delta = (1usize.wrapping_sub(residue)) & none_mask;
        let mut first_right = anchor as isize + right_delta as isize;
        if right_delta == 0 { first_right += stride };
        // `right_first` tie break - we scan right first if the element is closer to a right aligned spot
        //  than a left, but if theyre equidistant we go with bias.
        let right_first = right_delta * 2 < stride as usize || 
            (right_delta*2 == stride as usize && bias == Bias::Right);
        ScanParameters { first_right, first_left: first_right - stride, stride, right_delta, right_first }
    }
}



/// Outcome of exhausting one direction's probes without finding a `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// Reached the physical end with a representable push address.
    Viable,
    /// Reached the physical end but the push address is not representable.
    AddrLimit,
    /// Spent the stride budget before reaching any end.
    BudgetOut,
}

/// Map the two per-direction outcomes to a final `Result`. When BOTH ends are
/// viable, `bias` picks the side (Right→Append/back, Left→Prepend/front); a
/// single viable end is taken regardless of bias (no choice to make).
#[inline]
pub(crate) fn resolve(bias: Bias, right: End, left: End) -> Result<Found, NotFound> {
    match (right, left) {
        (End::Viable, End::Viable) => match bias {
            Bias::Right => Ok(Found::Append),
            Bias::Left => Ok(Found::Prepend),
        },
        (End::Viable, _) => Ok(Found::Append),
        (_, End::Viable) => Ok(Found::Prepend),
        (End::AddrLimit, _) | (_, End::AddrLimit) => Err(NotFound::AddressExhaustion),
        _ => Err(NotFound::OutOfBudget),
    }
}

/// Classify how each direction ended and combine into the final `Result`.
/// `bias` decides the both-ends-viable case (see [`resolve`]).
#[inline]
pub(crate) fn resolve_ends(bias: Bias,
                           count_right: usize,
                           count_left: usize,
                           budget: usize,
                           addr_shift: u32,
                           v_offset: usize,
                           addr_min: isize,
                           addr_max: isize,
                           len: usize)
                           -> Result<Found, NotFound> {
    let end_right = if count_right > budget {
        End::BudgetOut
    } else if back_viable(len, addr_shift, v_offset, addr_max) {
        End::Viable
    } else {
        End::AddrLimit
    };
    let end_left = if count_left > budget {
        End::BudgetOut
    } else if front_viable(addr_shift, v_offset, addr_min) {
        End::Viable
    } else {
        End::AddrLimit
    };
    resolve(bias, end_right, end_left)
}

/// Two-slice view over a `VecDeque`'s backing storage, taken once via
/// `as_slices` so the per-probe hot path is a slice deref, not `VecDeque`'s
/// `Index` impl (a `(head+i) % cap` modulo + bounds check per probe — `cap`
/// isn't power-of-two, so the modulo won't fold to an AND). Logical index `i`
/// routes with one compare; `front` precedes `back`.
pub(crate) struct Slots<'a, T> {
    pub(crate) front: &'a [Option<T>],
    pub(crate) back:  &'a [Option<T>],
}

impl<'a, T> Slots<'a, T> {
    /// Lookup logical index `i`. Caller guarantees `i < len`
    /// (`front.len() + back.len()`): every probe in `Block::find_slot`/`scan`
    /// is bounded by the in-range eligible-slot count, so positions stay in
    /// `[0, len)`. `get_unchecked` is sound under that invariant and skips the
    /// per-probe bounds check LLVM won't hoist out of the two-slice branch.
    /// The `back.is_empty()` fast path is loop-invariant → hoisted and
    /// unswitched; the common non-wrapped deque hits a single contiguous slice.
    #[inline]
    pub(crate) unsafe fn at_unchecked(&self, i: usize) -> &Option<T> {
        if self.back.is_empty() {
            unsafe { self.front.get_unchecked(i) }
        } else {
            let split = self.front.len();
            if i < split {
                unsafe { self.front.get_unchecked(i) }
            } else {
                unsafe { self.back.get_unchecked(i - split) }
            }
        }
    }
}

/// Step `delta` from `start` for `count` eligible slots; return the first `None`
/// index, or `None` if every probe is `Some`. `count` is capped to the in-range
/// eligible-slot count, so `start` and each stepped `pos` stay in `[0, len)`.
#[inline]
pub(crate) fn scan<T>(slots: &Slots<'_, T>,
                      start: isize,
                      delta: isize,
                      count: usize)
                      -> Option<usize> {
    let mut pos = start;
    for _ in 0..count {
        if unsafe { slots.at_unchecked(pos as usize) }.is_none() {
            return Some(pos as usize);
        }
        pos += delta;
    }
    None
}

#[cfg(test)]
#[path = "tests/find_slot.rs"]
mod tests;
