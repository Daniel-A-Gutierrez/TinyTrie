use std::fmt;

use crate::nibble::Nibble;
use crate::translator::Translator;

pub const MAX_CAP: usize = Nibble::CAP;

/// block payload: `v` is the self/vaddr (the pointer field — `fixup` re-stamps it to
/// `p2v(phys)` so the round-trip invariant holds after a move); `val` is the ordering
/// payload that travels with the node (parent order is by `val`). Off-midpoint rotation
/// makes the `v`s read e.g. `6,14,7,15` (disordered) while the `val`s stay ordered — fine,
/// since lookups go through `v`/`v2p` and the tree orders on `val`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub v:   u8,
    pub val: u8,
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.val)
    }
}
impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(v={}/{})", self.v, self.val)
    }
}

/// child of `old`: rotate + re-anchor `inner` so `p2v(0) == old.p2v(first_phys)` (first
/// element at phys 0). `p2v` on `old` (pre-rotate); only `rotate_left` uses the new rotation.
/// Asserts `old.shift == 0`.
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

/// child of `old`: rotate + re-anchor `inner` so `p2v(MAX_CAP - 1) == old.p2v(last_phys)`
/// (last element at the top phys). Mirror of `rsplit_translator` used for the right-bigger
/// half of an off-midpoint split: anchoring the last element there keeps the wrap from
/// landing in the low `[0, excess*2)` range the choreography pre-fills. Asserts `old.shift == 0`.
fn rsplit_translator_last(old: Translator, last_phys: usize) -> Translator {
    assert!(old.shift == 0, "cannot rotate a non-zero shift");
    let mut t = old;
    t.rotate();
    t.inner_offset = old
        .p2v(Nibble::from_usize(last_phys))
        .wrapping_sub(old.outer_offset)
        .rotate_left(t.rotation)
        .wrapping_shr(old.shift)
        .wrapping_sub(Nibble::from_usize(MAX_CAP - 1));
    t
}

/// `Vec<Option<T>>` of `len` slots + translator + `cap` (max len). Operates on phys slots
/// (translate vaddr via `translator().v2p()`). `len = min(MAX_CAP>>shift, cap)` — can be
/// full at `shift > 0` (must split, not spread).
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
    /// `len = min(MAX_CAP >> shift, cap)`; `cap` is the max len (grow via `spread`).
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

    /// root: `shift = bw`, len 1, cap MAX_CAP, no offsets. Grows by spread; full at `shift == 0`.
    pub fn uniform() -> Self {
        let tr = Translator::new(Nibble::ZERO, Nibble::ZERO, Nibble::BIT_WIDTH as u32, 0);
        Self { buf: vec![None], translator: tr, occupancy: 0, cap: MAX_CAP }
    }

    /// root: `shift = 2`, len 1, cap 4, outer = 8; canonical set wraps `{8,12,0,4}`.
    /// Grows by push (shift constant); full at `len == cap`. Built directly (under-filled
    /// vs `new`); push_front bumps outer since inner overflows at `shift > 0`.
    pub fn pluripotent() -> Self {
        let tr = Translator::new(Nibble::ZERO, Nibble::from_u8(8), 2, 0);
        Self { buf: vec![None], translator: tr, occupancy: 0, cap: 4 }
    }

    pub fn translator(&self) -> &Translator {
        &self.translator
    }

    /// max len, independent of `shift`.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// slot count; grows via `spread`.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// no occupied slots.
    pub fn is_vacant(&self) -> bool {
        self.occupancy == 0
    }

    /// count of occupied slots.
    pub fn occupancy(&self) -> usize {
        self.occupancy
    }

    /// panics if `phys >= len` or occupied.
    pub fn insert(&mut self, phys: usize, item: T) {
        assert!(phys < self.len(), "insert: phys {phys} >= len {}", self.len());
        assert!(self.buf[phys].is_none(), "insert: phys {phys} already occupied");
        self.buf[phys] = Some(item);
        self.occupancy += 1;
    }

    pub fn get(&self, phys: usize) -> Option<&T> {
        self.buf.get(phys).and_then(|s| s.as_ref())
    }

    /// panics if `phys >= len` or empty.
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

    /// double `len`, move `i -> 2i+offset` (vaddrs stable). Panics if `shift == 0` or
    /// `2*len > cap` (must split); both fire before any mutation.
    pub fn spread(&mut self, offset: bool) {
        let len = self.len();
        assert!(2 * len <= self.cap, "spread: 2*len {len} > cap/2 (at cap, must split)");
        self.buf.reserve(len);
        for _ in 0..len {
            self.buf.push(None);
        }
        self.spread_in_place(offset, len);
    }

    /// `i -> 2i+offset` for `i in 0..count` (high-to-low, no clobber), no `len` growth.
    /// Caller guarantees dst `[count, 2*count)` empty and in-bounds.
    fn spread_in_place(&mut self, offset: bool, count: usize) {
        self.translator.spread(offset);
        for i in (0..count).rev() {
            let dst = 2 * i + offset as usize;
            self.buf[dst] = self.buf[i].take();
        }
    }

    /// shift split (`shift > 0`): right born with `shift - 1` + re-anchored at `p2v(at)`;
    /// left spreads `i -> 2i` into the emptied high half (no `len` growth). Safe at
    /// `shift > 0`: stride >= 2 gaps a real midpoint, never the 15/0 seam.
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
        // anchor: p2v_right(0) == p2v_old(at) => inner = (p2v(at)-outer).rol(rot) >> right_shift.
        let right_inner = old
            .p2v(Nibble::from_usize(at))
            .wrapping_sub(old.outer_offset)
            .rotate_left(old.rotation)
            .wrapping_shr(right_shift);
        let mut right = Block::new(
            Translator::new(right_inner, old.outer_offset, right_shift, old.rotation),
            self.cap,
        );

        // move high-half items [at, len) into the right block.
        for phys in at..self.len() {
            if self.get(phys).is_some() {
                let v = old.p2v(Nibble::from_usize(phys));
                right.insert(right.translator.v2p(v).as_usize(), self.remove(phys));
            }
        }
        // left spreads i -> 2i into the emptied high phys (no len growth).
        self.spread_in_place(false, at);
        right
    }
}

impl Block<Node> {
    /// dispatcher: `shift > 0` → `split_shift` (free low bit), else `split_and_rotate`
    /// (free high bit). `at` is the phys split point.
    pub fn split(&mut self, at: usize) -> Block<Node> {
        if self.translator.shift > 0 {
            self.split_shift(at)
        } else {
            self.split_and_rotate(at)
        }
    }

/// rotation split (`shift == 0`): left keeps `[0, at)`, right `[at, len)`; both rotate +
/// re-anchor (reclaim the high bit). Midpoint: standard drain + reinsert via each child's
/// `v2p` (both halves come out `val`-ordered). Off-midpoint the bigger half's rotation
/// disorder is fixed in place: drain the smaller half out, preemptively place the seam range
/// (the `excess*2` items) at its final end, then compact the rest to its rotated phys - one
/// `swap_open` + `fixup` per move, `v` re-stamped each step (no deferred fixups; parent
/// `val`-order maintained, no `val` comparison). The bigger child is anchored so its
/// `inner=0`: left-bigger first-anchored, right-bigger last-anchored (`v2p(v_last)=15`),
/// which keeps the wrap out of the pre-filled low range. Asserts `shift == 0`; panics if
/// off-midpoint excess `e = |at - mid| >= mid/2` (infeasible). Precondition: full,
/// `val`-ordered block (phys `j` holds the j-th parent item).
pub fn split_and_rotate(&mut self, at: usize) -> Block<Node> {
    assert!(at <= self.len(), "split_and_rotate: at {at} > len {}", self.len());
    assert!(at != 0, "split_and_rotate: at == 0 (empty arc)");
    assert!(self.translator.shift == 0, "split_and_rotate: shift != 0 (use split)");
    let old = self.translator;
    let mid = Nibble::MIDPOINT.as_usize();
    let len = self.len();

    if at == mid {
        let left_new = rsplit_translator(old, 0);
        let mut right = Block::new(old, self.cap);
        right.translator = rsplit_translator(old, at);
        self.translator = left_new;
        let items: Vec<(usize, Node)> = self
            .buf
            .iter_mut()
            .enumerate()
            .filter_map(|(p, s)| s.take().map(|t| (p, t)))
            .collect();
        self.occupancy = 0;
        for (phys, item) in items {
            let v = old.p2v(Nibble::from_usize(phys));
            if phys < at {
                self.insert(self.translator.v2p(v).as_usize(), item);
            } else {
                right.insert(right.translator.v2p(v).as_usize(), item);
            }
        }
        return right;
    }

    let e = if at > mid { at - mid } else { mid - at };
    assert!(
        e < mid / 2,
        "split_and_rotate: excess {e} >= mid/2 {} (infeasible off-midpoint)",
        mid / 2
    );
    let two_e = 2 * e;

    if at > mid {
        // left is bigger (self). drain the smaller right half out (first-anchored, ordered).
        let mut right = Block::new(old, self.cap);
        right.translator = rsplit_translator(old, at);
        for j in at..len {
            if let Some(item) = self.buf[j].take() {
                let v = old.p2v(Nibble::from_usize(j));
                right.insert(right.translator.v2p(v).as_usize(), item);
            }
        }
        self.translator = rsplit_translator(old, 0); // left, first-anchored (inner=0)
        // preemptive: seam range [at-2e, at) -> top phys [16-2e, 16), in order. high->low so
        // each target is free when reached (src phys = j; item j sits at phys j pre-move).
        for j in (at - two_e..at).rev() {
            self.swap_open(j, len - at + j);
        }
        // compact: the rest [0, at-2e) -> its rotated phys new.v2p(old.p2v(j)). high->low.
        for j in (0..at - two_e).rev() {
            let dst = self.translator.v2p(old.p2v(Nibble::from_usize(j))).as_usize();
            if dst != j {
                self.swap_open(j, dst);
            }
        }
        self.occupancy = self.buf.iter().filter(|s| s.is_some()).count();
        right
    } else {
        // right is bigger (self). drain the smaller left half out (first-anchored, ordered).
        let mut left = Block::new(old, self.cap);
        left.translator = rsplit_translator(old, 0);
        for j in 0..at {
            if let Some(item) = self.buf[j].take() {
                let v = old.p2v(Nibble::from_usize(j));
                left.insert(left.translator.v2p(v).as_usize(), item);
            }
        }
        self.translator = rsplit_translator_last(old, len - 1); // right, last-anchored (inner=0)
        // preemptive: first 2e [at, at+2e) -> low phys [0, 2e), in order. low->high.
        for j in at..at + two_e {
            self.swap_open(j, j - at);
        }
        // compact: the rest [at+2e, len) -> its rotated phys new.v2p(old.p2v(j)). low->high.
        for j in at + two_e..len {
            let dst = self.translator.v2p(old.p2v(Nibble::from_usize(j))).as_usize();
            if dst != j {
                self.swap_open(j, dst);
            }
        }
        self.occupancy = self.buf.iter().filter(|s| s.is_some()).count();
        // self holds the (fixed) right half; swap with `left` so self = left, return right.
        std::mem::swap(&mut self.buf, &mut left.buf);
        std::mem::swap(&mut self.translator, &mut left.translator);
        std::mem::swap(&mut self.occupancy, &mut left.occupancy);
        std::mem::swap(&mut self.cap, &mut left.cap);
        left
    }
}

    /// seed empty phys with `Node { v: p2v(phys), val: phys }` — `v` is the canonical vaddr
    /// (free to wrap; `v1 > v2` implies nothing), `val` is the phys-rank order key so the
    /// block is `val`-ordered for any `inner`. No growth.
    pub fn fill(&mut self) {
        for phys in 0..self.len() {
            if self.buf[phys].is_none() {
                let v = self.translator.p2v(Nibble::from_usize(phys)).as_u8();
                self.buf[phys] = Some(Node { v, val: phys as u8 });
                self.occupancy += 1;
            }
        }
    }

    /// relocate the item at `src` into the open slot `open` (swap), then `fixup` re-keys
    /// it to `p2v(open)` — off-midpoint fixup phase 2. Panics if `src` empty or `open`
    /// occupied/oob; occupancy unchanged.
    pub fn swap_open(&mut self, src: usize, open: usize) {
        assert!(src < self.len(), "swap_open: src {src} oob");
        assert!(open < self.len(), "swap_open: open {open} oob");
        assert!(self.buf[src].is_some(), "swap_open: src {src} empty");
        assert!(self.buf[open].is_none(), "swap_open: open {open} occupied");
        self.buf.swap(src, open); // item relocates src→open, src freed
        self.fixup(open);
    }

    /// re-key the `v` field of the node at `phys` to `p2v(phys)` (the pointer field) —
    /// `val` is untouched. The re-key hook `compact_*`/`swap_open`/`off_midpoint_fixup_left`
    /// apply after each move; a real node impl recomputes the key vs the new vaddr (here
    /// `v` is the key). Panics if `phys >= len` or empty.
    pub fn fixup(&mut self, phys: usize) {
        assert!(phys < self.len(), "fixup: phys {phys} >= len {}", self.len());
        assert!(self.buf[phys].is_some(), "fixup: phys {phys} empty");
        let new_v = self.translator.p2v(Nibble::from_usize(phys)).as_u8();
        self.buf[phys].as_mut().unwrap().v = new_v;
    }

    /// carve `n` free at front (`[0, n)`): bubble the nearest `n` frees left via
    /// `None`↔`Some` swaps (stable, minimal — trailing gaps stay), then `fixup` the moved
    /// range `[n, last_r+1)` so the round-trip invariant holds. `[0,x,1,x,2,x,3]+2 →
    /// [x,x,0,1,2,x,3]`. Panics if `n > len` or fewer than `n` free.
    pub fn compact_left(&mut self, n: usize) {
        let len = self.len();
        assert!(n <= len, "compact_left: n {n} > len {len}");
        assert!(
            len - self.occupancy >= n,
            "compact_left: only {} free, need {n}",
            len - self.occupancy
        );
        let mut placed = 0;
        let mut last_r = 0; // origin of the n-th free (last relocated)
        for r in 0..len {
            if placed == n {
                break;
            }
            if self.buf[r].is_none() {
                for j in (placed..r).rev() {
                    self.buf.swap(j, j + 1);
                }
                placed += 1;
                last_r = r;
            }
        }
        if n > 0 {
            for phys in n..last_r + 1 {
                self.fixup(phys);
            }
        }
    }

    /// mirror of `compact_left`: carve `n` free at back (`[len-n, len)`), bubble the
    /// nearest `n` frees right, then `fixup` the moved range `[first_r, len-n)`. Panics
    /// if `n > len` or fewer than `n` free.
    pub fn compact_right(&mut self, n: usize) {
        let len = self.len();
        assert!(n <= len, "compact_right: n {n} > len {len}");
        assert!(
            len - self.occupancy >= n,
            "compact_right: only {} free, need {n}",
            len - self.occupancy
        );
        let mut placed = 0;
        let mut first_r = 0; // leftmost relocated free
        for r in (0..len).rev() {
            if placed == n {
                break;
            }
            if self.buf[r].is_none() {
                for j in r..len - 1 - placed {
                    self.buf.swap(j, j + 1);
                }
                placed += 1;
                first_r = r;
            }
        }
        if n > 0 {
            for phys in first_r..len - n {
                self.fixup(phys);
            }
        }
    }

    /// `p2v(len)` — next `push_back` vaddr.
    pub fn back_vaddr(&self) -> Nibble {
        self.translator.p2v(Nibble::from_usize(self.len()))
    }

    /// `p2v(0)` after the offset is bumped — next `push_front` vaddr.
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

    /// insert `v` (as `Node { v, val: v }`) at `v2p(v)`, auto-spread + retry on conflict
    /// (drives own growth). Panics at cap (must split).
    pub fn put(&mut self, v: u8) {
        let vn = Nibble::from_u8(v);
        let node = Node { v, val: v };
        loop {
            let phys = self.translator.v2p(vn).as_usize();
            if phys < self.len() && self.buf[phys].is_none() {
                self.buf[phys] = Some(node);
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

    /// append at phys `len` (vaddr `p2v(len)`); no translator change. `item` should be
    /// `back_vaddr()`. Panics at cap.
    pub fn push_back(&mut self, item: u8) {
        assert!(
            self.len() < self.cap,
            "push_back: at cap (len={}, cap={}), must split",
            self.len(),
            self.cap
        );
        self.buf.push(Some(Node { v: item, val: item }));
        self.occupancy += 1;
    }

    /// prepend at phys 0: bump offset down one stride (`inner -= 1` at `shift == 0`,
    /// else `outer -= 1<<shift`), shift `i -> i+1`. vaddrs preserved. `item` should be
    /// `front_vaddr()`. Panics at cap.
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
        self.buf[0] = Some(Node { v: item, val: item });
        self.occupancy += 1;
    }
}
