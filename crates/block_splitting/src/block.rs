use std::fmt;

use crate::nibble::Nibble;
use crate::translator::Translator;

pub const MAX_CAP: usize = Nibble::CAP;

/// Debug block: a `Vec<Option<T>>` of `len` slots + a `Translator` + a `cap` (max `len`).
/// Operates on **phys** slots; the caller translates vaddr -> phys via `translator().v2p()`.
/// `len = min(MAX_CAP >> shift, cap)`, so `log2(len) + shift <= bit_width` — len is decoupled
/// from shift, and a cap-constrained block can be full at `shift > 0` (must split, not spread).
#[derive(Clone)]
pub struct Block<T> {
    buf: Vec<Option<T>>,
    translator: Translator,
    occupancy: usize,
    cap: usize,
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
        assert!(cap >= 1 && cap <= MAX_CAP, "new: cap {cap} out of [1, {MAX_CAP}]");
        let len = (MAX_CAP >> translator.shift).min(cap);
        Self { buf: (0..len).map(|_| None).collect(), translator, occupancy: 0, cap }
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

    pub fn get_mut(&mut self, phys: usize) -> Option<&mut T> {
        self.buf.get_mut(phys).and_then(|s| s.as_mut())
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
        self.translator.spread(offset);
        self.buf.reserve(len);
        for _ in 0..len {
            self.buf.push(None);
        }
        // high-to-low: dst (2i+offset) > i > any unprocessed j, so take() never clobbers.
        for i in (0..len).rev() {
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
        // re-anchored at its first phys: inner = (p2v(first) - outer).rol(new_rot) >> shift.
        let mut left_new = old;
        left_new.rotate();
        left_new.inner_offset = old
            .p2v(Nibble::from_usize(from))
            .wrapping_sub(old.outer_offset)
            .rotate_left(left_new.rotation)
            .wrapping_shr(old.shift);
        let mut right = Block::new(old, self.cap);
        right.translator.rotate();
        right.translator.inner_offset = old
            .p2v(Nibble::from_usize(to))
            .wrapping_sub(old.outer_offset)
            .rotate_left(right.translator.rotation)
            .wrapping_shr(old.shift);

        // buffer both halves: drain all items, then re-insert into the anchored children
        // (no in-place move, so wrapping arcs and scattered dests are fine).
        let items: Vec<(usize, T)> = self
            .buf
            .iter_mut()
            .enumerate()
            .filter_map(|(p, s)| s.take().map(|t| (p, t)))
            .collect();
        self.occupancy = 0;

        let in_left = |p: usize| if from < to { p >= from && p < to } else { p >= from || p < to };
        for (phys, item) in items {
            let v = old.p2v(Nibble::from_usize(phys));
            if in_left(phys) {
                self.insert(left_new.v2p(v).as_usize(), item);
            } else {
                right.insert(right.translator.v2p(v).as_usize(), item);
            }
        }
        self.translator = left_new;
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
        let mut right = Block::new(Translator::new(right_inner, old.outer_offset, right_shift, old.rotation), self.cap);

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
        self.translator.spread(false);
        for i in (0..at).rev() {
            self.buf[2 * i] = self.buf[i].take();
        }
        right
    }

    // Original non-wrapping approach (in-place left move + drain), preserved for reference.
    // It avoids buffering by moving the left half in place and draining only the wrap-
    // clobbered middle [mid, at) where mid = v2p(MIDPOINT). Correct only when the canonical
    // set is phys-contiguous from 0 (inner == 0, or no wrap); split_from_to above buffers
    // both halves and handles wrapping arcs, and is the live implementation.
    //
    // pub fn split_and_rotate(&mut self, at: usize) -> Block<T> {
    //     assert!(at <= self.len());
    //     assert!(self.len() <= MAX_CAP >> self.translator.shift);
    //     let mid = self.translator.v2p(Nibble::MIDPOINT).as_usize();
    //     let mut left_new = self.translator.clone(); left_new.rotate();
    //     let mut right = Block::new(self.translator.clone()); right.translator.rotate();
    //     let tr = &self.translator;
    //     left_new.inner_offset = tr.p2v(Nibble::ZERO).wrapping_sub(tr.outer_offset)
    //         .rotate_left(left_new.rotation).wrapping_shr(tr.shift);
    //     right.translator.inner_offset = tr.p2v(Nibble::from_usize(at)).wrapping_sub(tr.outer_offset)
    //         .rotate_left(right.translator.rotation).wrapping_shr(tr.shift);
    //     for phys in at..self.len() { if self.get(phys).is_some() {
    //         let r = right.translator.v2p(self.translator.p2v(Nibble::from_usize(phys))).as_usize();
    //         right.insert(r, self.remove(phys));
    //     }}
    //     let mut side = Vec::with_capacity(at.saturating_sub(mid));
    //     for phys in mid..at { if self.get(phys).is_some() { side.push(Some(self.remove(phys))); }
    //         else { side.push(None) } }
    //     for phys in (0..mid).into_iter().rev() { if self.get(phys).is_some() {
    //         let r = left_new.v2p(self.translator.p2v(Nibble::from_usize(phys))).as_usize();
    //         self.insert(r, self.remove(phys));
    //     }}
    //     for (i, item) in side.into_iter().enumerate() {
    //         let d = left_new.v2p(self.translator.p2v(Nibble::from_usize(mid + i))).as_usize();
    //         if let Some(item) = item { self.insert(d, item); }
    //     }
    //     self.translator = left_new; right
    // }
}