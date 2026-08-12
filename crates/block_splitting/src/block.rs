use std::fmt;

use crate::nibble::Nibble;
use crate::translator::Translator;

pub const MAX_CAP: usize = Nibble::CAP;

/// Debug block: a `Vec<Option<T>>` of `len` slots (growing via `spread` to
/// `MAX_CAP`) + a `Translator`. Operates on **phys** slots; the caller translates
/// vaddr -> phys via `translator().v2p()`. Mirrors doa's block shape without
/// grow/spread/split/strategy plumbing beyond spread itself.
#[derive(Clone)]
pub struct Block<T> {
    buf: Vec<Option<T>>,
    translator: Translator,
    occupancy: usize,
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
    /// `len` starts at the translator's canonical capacity (`MAX_CAP >> shift`) and
    /// grows via `spread` until it reaches `MAX_CAP`.
    pub fn new(translator: Translator) -> Self {
        assert!(
            translator.shift <= Nibble::BIT_WIDTH as u32,
            "shift {} > {}",
            translator.shift,
            Nibble::BIT_WIDTH
        );
        let len = MAX_CAP >> translator.shift;
        Self { buf: (0..len).map(|_| None).collect(), translator, occupancy: 0 }
    }

    pub fn translator(&self) -> &Translator {
        &self.translator
    }

    /// fixed max capacity (16).
    pub fn cap(&self) -> usize {
        MAX_CAP
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
    /// Panics if `shift == 0` (no room to drop shift) or `len > MAX_CAP/2`
    /// (doubling would exceed `MAX_CAP`); both fire before any mutation.
    pub fn spread(&mut self, offset: bool) {
        let len = self.len();
        assert!(len <= MAX_CAP / 2, "spread: len {len} > {}", MAX_CAP / 2);
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

    /// Split a full block (`shift == 0`) at `at` (`0 < at < len`): `self` becomes
    /// the LEFT half (phys `0..at`), the RIGHT half (phys `at..len`) splits off.
    /// Both halves bump `rotation` by 1. Each moved item is placed at
    /// `new.v2p(old.p2v(phys))`, so it lands where the rotated translator decodes
    /// its own vaddr — the local invariant `v2p(item) == phys` holds per-item.
    /// Pitfall: the left half moves in place, so an item can land on a not-yet-
    /// vacated slot and panic "already occupied" depending on `at` and rotation.
    pub fn split_and_rotate(&mut self, at: usize) -> Block<T> {
        assert!(at <= self.len(), "split_and_rotate: at {at} > len {}", self.len());
        // rotation makes v2p `2^shift`-to-1 with collision pairs {v, v+8}; splitting at the
        // vaddr midpoint puts one of each pair in each half, so each child's items stay in
        // distinct phys. Bounded by the canonical capacity so the block isn't over-full.
        assert!(
            self.len() <= MAX_CAP >> self.translator.shift,
            "split_and_rotate: len {} > {}",
            self.len(),
            MAX_CAP >> self.translator.shift
        );
        let mid = self.translator.v2p(Nibble::MIDPOINT).as_usize();
        // right.translator is rotated first; it holds the NEW mapping used to place
        // every item. self.translator stays OLD through the loops so p2v recovers the
        // original vaddr; it's rotated last to match the placements.
        let mut right = Block::new(self.translator.clone());
        right.translator.rotate();
        // right partition [at, len) -> right block at its rotated phys.
        for phys in at..self.len() {
            if self.get(phys).is_some() {
                let rot_phys = right.translator.v2p(self.translator.p2v(Nibble::from_usize(phys))).as_usize();
                right.insert(rot_phys, self.remove(phys));
            }
        }
        // drain the wrap-clobbered middle [mid, at) into a side buffer first; its items
        // rotate into low odd slots the in-place loop below would otherwise overwrite.
        let mut side = Vec::with_capacity(at.saturating_sub(mid));
        for phys in mid..at {
            if self.get(phys).is_some() {
                side.push(Some(self.remove(phys)));
            }
            else { side.push(None) }
        }
        // left lower part [0, mid) moves in place (targets are even slots 2i >= i; the
        // >= mid ones were just emptied by the drain / the right drain above).
        for phys in (0..mid).into_iter().rev() {
            if self.get(phys).is_some() {
                let rot_phys = right.translator.v2p(self.translator.p2v(Nibble::from_usize(phys))).as_usize();
                let moved = self.remove(phys);
                self.insert(rot_phys, moved);
            }
        }
        // reinsert the drained middle at its rotated phys (orig phys = mid + i).
        for (i, item) in side.into_iter().enumerate() {
            let dest = right.translator.v2p(self.translator.p2v(Nibble::from_usize(mid + i))).as_usize();
            if let Some(item) = item {
                self.insert(dest, item);
            }
        }
        self.translator.rotate();
        right
    }
}