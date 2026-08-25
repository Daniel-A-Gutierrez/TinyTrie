use std::fmt;

use crate::nibble::Nibble;
use crate::translator::Translator;

pub const MAX_CAP: usize = Nibble::CAP;

/// Build a child translator from `old`: bump `rotation` by 1 and re-anchor `inner_offset`
/// so the child's `p2v(0) == old.p2v(first_phys)`. `outer`/`shift` inherited. `p2v` is
/// evaluated on `old` (pre-rotate); only the `rotate_left` uses the new rotation.
fn anchored(old: Translator, first_phys: usize) -> Translator {
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

    /// General split: cut the canonical cyclic phys sequence into two arcs and rotate
    /// each into its own block. `self` keeps the `[from, to)` arc (LEFT); the `[to, from)`
    /// arc (RIGHT) splits off. `from > to` means the arc wraps: `[from, len) ∪ [0, to)`.
    /// Both children bump `rotation` by 1 and re-anchor so their first vaddr lands at
    /// phys 0. Items are buffered (both halves), so this handles wrapping canonical sets
    /// (`shift > 0` + `inner != 0`) that the non-wrapping approach below can't.
    ///
    /// For `shift > 0` the cut must separate collision pairs `{v, v+8}` — `from`/`to` at
    /// the vaddr-8 crossings — or the children collide (`insert` panics). For `shift == 0`
    /// any `from != to` works.
    pub fn split_from_to(&mut self, from: usize, to: usize) -> Block<T> {
        assert!(from <= self.len(), "split_from_to: from {from} > len {}", self.len());
        assert!(to <= self.len(), "split_from_to: to {to} > len {}", self.len());
        assert!(from != to, "split_from_to: from == to (empty/full arc)");
        assert!(
            self.len() <= MAX_CAP >> self.translator.shift,
            "split_from_to: len {} > {}",
            self.len(),
            MAX_CAP >> self.translator.shift
        );

        let old = self.translator;
        // left child = arc [from, to), right child = arc [to, from). Each rotated +
        // re-anchored at its first phys so its first vaddr lands at phys 0.
        let left_new = anchored(old, from);
        let mut right = Block::new(old, self.cap);
        right.translator = anchored(old, to);
        self.translator = left_new;

        // buffer both halves: drain all items, then re-insert into the anchored children
        // (no in-place move, so wrapping arcs and scattered dests are fine).
        let items: Vec<(usize, T)> = self
            .buf
            .iter_mut()
            .enumerate()
            .filter_map(|(p, s)| s.take().map(|t| (p, t)))
            .collect();
        self.occupancy = 0;

        let in_left =
            |p: usize| if from < to { p >= from && p < to } else { p >= from || p < to };
        for (phys, item) in items {
            let v = old.p2v(Nibble::from_usize(phys));
            if in_left(phys) {
                let dst = self.translator.v2p(v).as_usize();
                self.insert(dst, item);
            } else {
                right.insert(right.translator.v2p(v).as_usize(), item);
            }
        }
        right
    }

    /// Non-wrapping split at `at`: left = phys `[0, at)`, right = phys `[at, len)`.
    /// Thin special case of `split_from_to(0, at)`.
    pub fn split_and_rotate(&mut self, at: usize) -> Block<T> {
        self.split_from_to(0, at)
    }

    /// Split a block that still has room to shift (`shift > 0`): the RIGHT half (phys
    /// `[at, len)`) splits off into a new block with `shift -= 1` (more room, stride gaps)
    /// and `inner` re-anchored so its first vaddr `p2v(at)` lands at phys 0. Then the LEFT
    /// (`self`) spreads (`shift -= 1`, items to stride-2) so it also has room to grow. No
    /// rotation — the shift decreases interleave the empty space. Use `split_and_rotate`
    /// for full (`shift == 0`) blocks.
    pub fn split_and_shift(&mut self, at: usize) -> Block<T> {
        assert!(at <= self.len(), "split_and_shift: at {at} > len {}", self.len());
        assert!(
            self.translator.shift > 0,
            "split_and_shift: shift == 0 (use split_and_rotate)"
        );
        assert!(
            self.len() <= MAX_CAP >> self.translator.shift,
            "split_and_shift: len {} > {}",
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
        // growth. The high phys [at, len) were just emptied by the split, providing the space
        // (a cap-constrained block can't grow len, so we don't call `spread` which doubles it).
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
