use std::fmt;

use crate::nibble::Nibble;
use crate::translator::Translator;

pub const MAX_CAP: usize = Nibble::CAP;

/// Build a child translator from `old`: bump `rotation` by 1 and re-anchor `inner_offset`
/// so the child's `p2v(0) == old.p2v(first_phys)`. `outer`/`shift` inherited. `p2v` is
/// evaluated on `old` (pre-rotate); only the `rotate_left` uses the new rotation.
fn rsplit_translator(old: Translator, first_phys: usize) -> Translator {
    assert!(old.shift == 0, "cannot rotate a non-zero shift");
    let mut t = old;
    t.rotate();
    t.inner_offset = old
        .p2v(Nibble::from_usize(first_phys))
        .wrapping_sub(old.outer_offset)
        .rotate_left(t.rotation)
        .wrapping_shr(old.shift);
    t
}

/// Debug block: a `Vec<Option<T>>` of `len` slots + a `Translator` + a `cap` (max `len`).
/// Operates on **phys** slots; the caller translates vaddr -> phys via `translator().v2p()`.
/// `len = min(MAX_CAP >> shift, cap)`, so `log2(len) + shift <= bit_width` — len is decoupled
/// from shift, and a cap-constrained block can be full at `shift > 0` (must split, not spread).
#[derive(Clone)]
pub struct Block<T> {
    buf:        Vec<Option<T>>,
    translator: Translator,
    occupancy:  usize,
    cap:        usize,
}

impl<T: fmt::Display> fmt::Display for Block<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "translator: {}", self.translator)?;
        write!(f, "[")?;
        for (i, slot) in self.buf.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            match slot {
                None => write!(f, "N")?,
                Some(t) => write!(f, "{t}")?,
            }
        }
        write!(f, "]")
    }
}

impl<T: fmt::Debug> fmt::Debug for Block<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("buf", &self.buf)
            .field("translator", &self.translator)
            .finish()
    }
}

impl<T> Block<T> {
    /// `len = min(MAX_CAP >> shift, cap)`. `cap` is the max `len` (the block grows via
    /// `spread` up to `cap`); `cap <= MAX_CAP` so `log2(len) + shift <= bit_width`.
    pub fn new(translator: Translator, cap: usize) -> Self {
        assert!(
            translator.shift <= Nibble::BIT_WIDTH as u32,
            "shift {} > {}",
            translator.shift,
            Nibble::BIT_WIDTH
        );
        assert!((1..=MAX_CAP).contains(&cap), "new: cap {cap} out of [1, {MAX_CAP}]");
        let len = (MAX_CAP >> translator.shift).min(cap);
        Self { buf: (0..len).map(|_| None).collect(), translator, occupancy: 0, cap }
    }

    /// Uniform root: `shift = bw` (len 1), `cap = MAX_CAP`, no offsets. Grows purely by
    /// spread (insert auto-spreads); never push_front/back. Full at `shift == 0`.
    pub fn uniform() -> Self {
        let tr = Translator::new(Nibble::ZERO, Nibble::ZERO, Nibble::BIT_WIDTH as u32, 0);
        Self { buf: vec![None], translator: tr, occupancy: 0, cap: MAX_CAP }
    }

    /// Pluripotent root: `shift = 2`, `len = 1`, `cap = 4`, `inner = 0`, `outer = 8`.
    /// The offset lives in OUTER (vaddr) space: at `shift > 0` decrementing inner would
    /// overflow and break v2p round-trip, so push_front bumps outer instead. Canonical
    /// set wraps (`{8,12,0,4}`), leaving room on both ends — front/back/middle all work.
    /// Grows by push (len up, shift constant); full at `len == cap` while `shift > 0`.
    ///
    /// Built directly (not via `new`): `new` would size `len = min(MAX_CAP>>2, 4) = 4`, but
    /// this root starts at `len = 1` and grows by `push_*`. `new`'s `len` formula is the
    /// *full* size for a given translator; these roots deliberately start under-filled.
    pub fn pluripotent() -> Self {
        let tr = Translator::new(Nibble::ZERO, Nibble::from_u8(8), 2, 0);
        Self { buf: vec![None], translator: tr, occupancy: 0, cap: 4 }
    }

    pub fn translator(&self) -> &Translator {
        &self.translator
    }

    /// max `len` (buf size cap), independent of `shift`.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// current number of slots (live region `[0, len)`). Grows via `spread`.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// no `Some` slots.
    pub fn is_vacant(&self) -> bool {
        self.occupancy == 0
    }

    /// number of `Some` slots. Maintained by `insert`/`remove`; `spread` preserves it.
    pub fn occupancy(&self) -> usize {
        self.occupancy
    }

    /// place `item` at phys slot `phys`. Panics if `phys >= len` or the slot is occupied.
    pub fn insert(&mut self, phys: usize, item: T) {
        assert!(phys < self.len(), "insert: phys {phys} >= len {}", self.len());
        assert!(self.buf[phys].is_none(), "insert: phys {phys} already occupied");
        self.buf[phys] = Some(item);
        self.occupancy += 1;
    }

    pub fn get(&self, phys: usize) -> Option<&T> {
        self.buf.get(phys).and_then(|s| s.as_ref())
    }

    /// remove and return the item at phys slot `phys`. Panics if `phys >= len` or the slot is empty.
    pub fn remove(&mut self, phys: usize) -> T {
        assert!(phys < self.len(), "remove: phys {phys} >= len {}", self.len());
        let item = self.buf[phys].take();
        assert!(item.is_some(), "remove: phys {phys} empty");
        self.occupancy -= 1;
        item.unwrap()
    }

    /// occupied (phys, &item) pairs in phys order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.buf.iter().enumerate().filter_map(|(p, s)| s.as_ref().map(|t| (p, t)))
    }

    /// Double `len`: grow the buf to `2*len`, move each phys `i -> 2i + offset`,
    /// then let the translator update its params (preserving handed-out vaddrs).
    /// Panics if `shift == 0` (translator.spread) or `2*len > cap` (at the cap, must
    /// split, not spread); both fire before any mutation.
    pub fn spread(&mut self, offset: bool) {
        let len = self.len();
        assert!(2 * len <= self.cap, "spread: 2*len {len} > cap/2 (at cap, must split)");
        self.buf.reserve(len);
        for _ in 0..len {
            self.buf.push(None);
        }
        self.spread_in_place(offset, len);
    }

    /// Move each phys `i -> 2i + offset` for `i in 0..count` (high-to-low so `take()` never
    /// clobbers an unprocessed slot) and update translator params. No `len` growth — caller
    /// ensures the `[count, 2*count)` dst range is empty and in-bounds.
    fn spread_in_place(&mut self, offset: bool, count: usize) {
        self.translator.spread(offset);
        for i in (0..count).rev() {
            let dst = 2 * i + offset as usize;
            self.buf[dst] = self.buf[i].take();
        }
    }

    /// Rotation split (`shift == 0`): left keeps phys `[0, at)`, right takes phys
    /// `[at, len)`. Both children bump `rotation` by 1 and re-anchor so their first
    /// vaddr lands at phys 0 — reclaiming the now-constant high bit (rotated into the
    /// low position) as fresh inter-item gap space, the split-time dual of `spread`.
    /// Items are buffered and re-inserted via each child's `v2p`: no in-place
    /// `i -> 2i` move, which at stride 1 would mis-gap the 15/0 wrap seam (no vaddr
    /// lies between them). Panics if `shift != 0` (via `rsplit_translator`) — route through
    /// `split` to pick shift vs rotate automatically.
    pub fn split_and_rotate(&mut self, at: usize) -> Block<T> {
        assert!(at <= self.len(), "split_and_rotate: at {at} > len {}", self.len());
        assert!(at != 0, "split_and_rotate: at == 0 (empty arc)");
        assert!(
            self.len() <= MAX_CAP >> self.translator.shift,
            "split_and_rotate: len {} > {}",
            self.len(),
            MAX_CAP >> self.translator.shift
        );

        let old = self.translator;
        // left child anchored at phys 0, right child at phys `at`. Each rotated +
        // re-anchored so its first vaddr lands at phys 0.
        let left_new = rsplit_translator(old, 0);
        let mut right = Block::new(old, self.cap);
        right.translator = rsplit_translator(old, at);
        self.translator = left_new;

        // buffer both halves: drain all items, then re-insert into the anchored children
        // (no in-place move, so the rotation remap handles every item uniformly).
        let items: Vec<(usize, T)> = self
            .buf
            .iter_mut()
            .enumerate()
            .filter_map(|(p, s)| s.take().map(|t| (p, t)))
            .collect();
        self.occupancy = 0;

        for (phys, item) in items {
            let v = old.p2v(Nibble::from_usize(phys));
            if phys < at {
                let dst = self.translator.v2p(v).as_usize();
                self.insert(dst, item);
            } else {
                right.insert(right.translator.v2p(v).as_usize(), item);
            }
        }
        right
    }

    /// Split dispatcher: pick the operation that reclaims the next gap bit. If
    /// `shift > 0` there's still a low bit to free, so shift (`split_shift`: right
    /// `shift -= 1`, left spreads `i -> 2i`). Else `shift == 0` — low bits exhausted —
    /// so rotate (`split_and_rotate`: give the top half to the sibling and reclaim the
    /// high bit). `at` is the split point in phys space.
    pub fn split(&mut self, at: usize) -> Block<T> {
        if self.translator.shift > 0 {
            self.split_shift(at)
        } else {
            self.split_and_rotate(at)
        }
    }

    /// Shift split (`shift > 0`): the RIGHT half (phys `[at, len)`) splits off into a
    /// new block with `shift -= 1` (stride gaps, no rotation) and `inner` re-anchored
    /// so its first vaddr `p2v(at)` lands at phys 0. Then the LEFT (`self`) spreads
    /// (`shift -= 1`, items `i -> 2i`) into the just-emptied high phys — no `len`
    /// growth (a cap-constrained block can't double). Safe at `shift > 0` because
    /// stride `1 << shift >= 2` means the 15/0 wrap seam is never a consecutive placed
    /// pair, so the spread always gaps a real midpoint vaddr.
    fn split_shift(&mut self, at: usize) -> Block<T> {
        assert!(at <= self.len(), "split_shift: at {at} > len {}", self.len());
        assert!(self.translator.shift > 0, "split_shift: shift == 0 (use split_and_rotate)");
        assert!(
            self.len() <= MAX_CAP >> self.translator.shift,
            "split_shift: len {} > {}",
            self.len(),
            MAX_CAP >> self.translator.shift
        );

        let old = self.translator;
        let right_shift = old.shift - 1;
        // anchor: p2v_right(0) == p2v_old(at) => inner = (p2v(at) - outer).rol(rot) >> right_shift.
        let right_inner = old
            .p2v(Nibble::from_usize(at))
            .wrapping_sub(old.outer_offset)
            .rotate_left(old.rotation)
            .wrapping_shr(right_shift);
        let mut right = Block::new(
            Translator::new(right_inner, old.outer_offset, right_shift, old.rotation),
            self.cap,
        );

        // left keeps its translator for now; move high-half items [at, len) into the right block.
        for phys in at..self.len() {
            if self.get(phys).is_some() {
                let v = old.p2v(Nibble::from_usize(phys));
                right.insert(right.translator.v2p(v).as_usize(), self.remove(phys));
            }
        }
        // in-place spread of the left: items phys i -> 2i (i in 0..at), shift -= 1, NO len
        // growth. The high phys [at, len) were just emptied by the split, providing the space.
        self.spread_in_place(false, at);
        right
    }
}

impl Block<u8> {
    /// Fill every empty phys in `[0, len)` with its decoded vaddr `p2v(phys)`, so
    /// `block[i] == p2v(i)` (and `v2p(block[i]) == i` on canonical phys). No growth.
    pub fn fill(&mut self) {
        for phys in 0..self.len() {
            if self.buf[phys].is_none() {
                let v = self.translator.p2v(Nibble::from_usize(phys)).as_u8();
                self.buf[phys] = Some(v);
                self.occupancy += 1;
            }
        }
    }

    /// vaddr the next `push_back` will occupy: `p2v(len)` (translator unchanged).
    pub fn back_vaddr(&self) -> Nibble {
        self.translator.p2v(Nibble::from_usize(self.len()))
    }

    /// vaddr the next `push_front` will occupy: `p2v(0)` after the offset is bumped
    /// (`inner -= 1` at `shift == 0`, `outer -= 1<<shift` at `shift > 0`).
    pub fn front_vaddr(&self) -> Nibble {
        let mut t = self.translator;
        let stride = 1u8 << t.shift;
        if t.shift == 0 {
            t.inner_offset = t.inner_offset.wrapping_sub(Nibble::ONE);
        } else {
            t.outer_offset = t.outer_offset.wrapping_sub(Nibble::from_u8(stride));
        }
        t.p2v(Nibble::ZERO)
    }

    /// Insert `v` (as payload) at `v2p(v)`. If the target phys is occupied or out of the
    /// live region, auto-spread (doubling `len`, dropping `shift`) and retry — so filling
    /// a growing block with canonical vaddrs drives its own growth. Panics at the cap
    /// (`len == cap` or `shift == 0` with no room): must split, not spread.
    pub fn put(&mut self, v: u8) {
        let vn = Nibble::from_u8(v);
        loop {
            let phys = self.translator.v2p(vn).as_usize();
            if phys < self.len() && self.buf[phys].is_none() {
                self.buf[phys] = Some(v);
                self.occupancy += 1;
                return;
            }
            assert!(
                self.translator.shift > 0 && self.len() < self.cap,
                "put: at cap (len={}, cap={}, shift={}), must split",
                self.len(),
                self.cap,
                self.translator.shift
            );
            self.spread(false);
        }
    }

    /// Append `item` at a new phys `len` (vaddr `p2v(len)`). No translator change — the
    /// new phys is already canonical for an inner=0 block growing within `cap`. Panics
    /// at the cap. `item` should be `back_vaddr()` for the invariant to hold.
    pub fn push_back(&mut self, item: u8) {
        assert!(
            self.len() < self.cap,
            "push_back: at cap (len={}, cap={}), must split",
            self.len(),
            self.cap
        );
        self.buf.push(Some(item));
        self.occupancy += 1;
    }

    /// Prepend `item` at phys 0: bump the offset down by one stride (`inner -= 1` at
    /// `shift == 0`; `outer -= 1<<shift` at `shift > 0`, since inner lives pre-shift and
    /// would overflow), shift existing items `phys i -> i+1`, grow `len` by 1. Existing
    /// vaddrs are preserved (offset down by stride, phys up by 1 ⇒ vaddr constant).
    /// `item` should be `front_vaddr()` for the invariant to hold. Panics at the cap.
    pub fn push_front(&mut self, item: u8) {
        assert!(
            self.len() < self.cap,
            "push_front: at cap (len={}, cap={}), must split",
            self.len(),
            self.cap
        );
        let stride = 1u8 << self.translator.shift;
        if self.translator.shift == 0 {
            self.translator.inner_offset =
                self.translator.inner_offset.wrapping_sub(Nibble::ONE);
        } else {
            self.translator.outer_offset =
                self.translator.outer_offset.wrapping_sub(Nibble::from_u8(stride));
        }
        let n = self.buf.len();
        self.buf.push(None);
        for i in (0..n).rev() {
            self.buf[i + 1] = self.buf[i].take();
        }
        self.buf[0] = Some(item);
        self.occupancy += 1;
    }
}
