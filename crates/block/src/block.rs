//!`Block<P>` — fixed-storage dual-front allocator: header + trailing data in
//!ONE allocation; `[stack | free | heap]` grow toward each other from the data
//!region's ends, every op O(1) arithmetic on two lengths.
//!invariants:
//!- serialized form = the raw allocation bytes; `Header` is exactly 16 bytes,
//!  `ptr` = 16, cap rounded to 16 — fixed-cap blocks are fixed-stride on disk.
//!- the allocation is 16-aligned and `ptr + cap` ≡ 0 mod 16, so BOTH data ends
//!  are absolutely 16-aligned; `align_of::<T>() > MAX_ALIGN` is a compile panic.
//!- zero-record invariant: every byte of the serialized record is LIVE DATA,
//!  hole bookkeeping, or ZERO — born zeroed (alloc_zeroed), `grow` zeroes the
//!  realloc'd tail (realloc's extension is garbage), reclaims wipe (`stack_shrink`,
//!  `pop_heap`/rewind — pruning any hole records they roll past), and `free`
//!  wipes the user bytes leaving an 8-byte hole header per hole. no stale bytes
//!  ever reach the record; a deleted item's bytes leave with it. `alloc` memsets
//!  nothing — regions claim zeroed gap
//!- the block NEVER drops: stack values transfer bitwise (`push` forgets),
//!  `stack_shrink` reclaims without drops, heap rolls back via `pop_heap`.
//!- space failures return `CapErr { free }` (bytes between the fronts), never
//!  panic; panics are contract violations only (malformed handles, misuse).
//!- `grow` reallocs and slides the heap tail to the new back: `S` (front-
//!  relative) and `H` (back-relative) resolve identically; every raw slice /
//!  `&T` view handed out before the grow is invalid; `ptr`/header stay put.
//!- `Block` is `!Send`/`!Sync` until a threading layer says otherwise.

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc};
use std::fmt;
use std::marker::PhantomData;
use std::mem::{align_of, size_of};
use std::ptr::{self, NonNull};
use std::slice;

use crate::handle::{H, Ptr, S};

///max served alignment — both data ends are kept 16-aligned for it.
pub const MAX_ALIGN: usize = 16;

const HEADER_SIZE: usize = 16;

#[repr(C, align(16))]
struct Header {
    stack: u32,
    heap: u32,
    cap: u32,
    ptr: u32,
}
const _: () = assert!(size_of::<Header>() == HEADER_SIZE);

///space failure; `free` = bytes still between the fronts (max headroom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapErr {
    pub free: usize,
}

///hole header magic — stamped at each freed region's start.
const HOLE_MAGIC: u32 = 0xF7EE_F7EE;

///rewind token for `Allocator::rewind` — the heap front plus the hole
///bookkeeping cloned at capture time (so a rewind erases the frees it spans).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub heap: usize,
    pub holes: Vec<(u32, u32)>,
}

impl fmt::Display for CapErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block capacity exceeded ({} bytes free)", self.free)
    }
}

impl std::error::Error for CapErr {}

pub struct Block<P> {
    base: NonNull<u8>,
    ///freed heap regions `(back_off, len)` — back_off = region start from the
    ///data back edge in H's convention; sorted ascending, pairwise
    ///non-touching (free coalesces). the Vec never serializes: a load
    ///rebuilds ≥ 8-byte holes from their in-heap stamps; sub-8 holes carry no
    ///stamp — the persistence boundary, runtime-only.
    holes: Vec<(u32, u32)>,
    #[cfg(test)]
    frees: Vec<H<P>>,
    _p: PhantomData<P>,
}

impl<P: Ptr> Block<P> {
    ///data region of `cap` bytes (rounded up to 16, the serialized stride).
    ///panics on cap beyond `P`'s or u32's width. region starts zeroed.
    pub fn new(cap: usize) -> Self {
        let cap = (cap.max(MAX_ALIGN) + MAX_ALIGN - 1) & !(MAX_ALIGN - 1);
        Self::check_cap(cap);
        let layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: size > 0, align is a power of two; zeroed per the
        // zero-record invariant
        let base = unsafe { alloc_zeroed(layout) };
        if base.is_null() {
            handle_alloc_error(layout)
        }
        // SAFETY: alloc_zeroed returned a live allocation of `layout`
        let mut b = Block {
            // SAFETY: base is non-null here
            base: unsafe { NonNull::new_unchecked(base) },
            holes: Vec::new(),
            #[cfg(test)]
            frees: Vec::new(),
            _p: PhantomData,
        };
        let h = b.hdr_mut();
        h.stack = 0;
        h.heap = 0;
        h.cap = cap as u32;
        h.ptr = HEADER_SIZE as u32;
        b
    }

    ///load seam: rebuilds from a serialized record — byte-for-byte, header
    ///included. caller guarantees a sane record (Stack::validate): len == 16
    ///+ cap, cap a 16-multiple inside `P`'s width.
    pub(crate) fn from_record(record: &[u8]) -> Self {
        let cap = u32::from_le_bytes([record[8], record[9], record[10], record[11]]) as usize;
        assert!(record.len() == HEADER_SIZE + cap, "record size != 16 + cap");
        Self::check_cap(cap);
        let layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: size > 0, align a power of two
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            handle_alloc_error(layout)
        }
        // SAFETY: fresh allocation — the record copies over header and data
        // region intact (zero-record invariant carried by the record itself)
        unsafe {
            ptr::copy_nonoverlapping(record.as_ptr(), base, record.len());
        }
        let mut b = Block {
            // SAFETY: base is non-null here
            base: unsafe { NonNull::new_unchecked(base) },
            holes: Vec::new(),
            #[cfg(test)]
            frees: Vec::new(),
            _p: PhantomData,
        };
        //the stamps rode the byte copy — rebuild the freelist from them
        b.holes = Self::discover_holes(record, cap, b.heap_len());
        b
    }

    ///rebuilds `holes` from the heap span's 8-byte stamps (MAGIC | len), in
    ///ascending back_off order. a MAGIC-shaped u32 is ~2^-32 per position on
    ///live data; the len/span bounds reject what little survives.
    fn discover_holes(record: &[u8], cap: usize, heap: usize) -> Vec<(u32, u32)> {
        let f4 = |i: usize| u32::from_le_bytes(record[i..i + 4].try_into().unwrap());
        let (lo, hi) = (16 + cap.saturating_sub(heap), 16 + cap);
        let mut found = Vec::new();
        let mut i = lo;
        while i + 8 <= hi {
            if f4(i) == HOLE_MAGIC {
                let len = f4(i + 4) as usize;
                if len >= 8 && i + len <= hi {
                    found.push(((cap - (i - 16)) as u32, len as u32));
                    i += 8; //skip the accepted header — its span is wiped zero
                    continue;
                }
            }
            i += 1;
        }
        found.reverse();
        found
    }

    fn check_cap(cap: usize) {
        assert!(
            cap <= u32::MAX as usize && cap <= P::MAX.as_usize(),
            "cap exceeds pointer width"
        );
    }

    ///grows to `new_cap` data bytes (rounded up to 16). ≤ cap is a no-op when
    ///the used bytes still fit, else an error. realloc + heap-tail slide:
    ///all `S`/`H` resolve identically, all previously handed-out raw slices
    ///and `&T` views die. panic-free on space: too-small → `CapErr`.
    pub fn grow(&mut self, new_cap: usize) -> Result<(), CapErr> {
        let used = self.stack_len() + self.heap_len();
        let new_cap = (new_cap + MAX_ALIGN - 1) & !(MAX_ALIGN - 1);
        let cap = self.cap();
        if new_cap <= cap {
            return if new_cap < used {
                Err(self.cap_err())
            } else {
                Ok(())
            };
        }
        Self::check_cap(new_cap);
        let old_layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: realloc of the owned block; on move, bytes persist — S (front-
        // relative) and H (back-relative) re-resolve unchanged, header rides along
        let base = unsafe {
            realloc(self.base.as_ptr(), old_layout, HEADER_SIZE + new_cap)
        };
        if base.is_null() {
            let layout =
                Layout::from_size_align(HEADER_SIZE + new_cap, MAX_ALIGN).expect("layout");
            handle_alloc_error(layout)
        }
        // SAFETY: realloc's extension is uninitialized — zero the new tail so
        // the zero-record invariant holds (the slide below may overlap it)
        unsafe { ptr::write_bytes(base.add(HEADER_SIZE + cap), 0, new_cap - cap) };
        // SAFETY: same invariants as hdr_mut; allocation moved under us
        let h = unsafe { &mut *base.cast::<Header>() };
        h.cap = new_cap as u32;
        // heap tail sits at old offsets: slide it back by the grown amount
        // (hole back_offsets are back-relative — invariant under the slide)
        let heap = h.heap as usize;
        // SAFETY: both ends inside the new allocation (new_cap > cap); ptr::copy
        // is memmove — source and destination overlap when heap > 0
        unsafe {
            ptr::copy(
                base.add(HEADER_SIZE + cap - heap),
                base.add(HEADER_SIZE + new_cap - heap),
                heap,
            );
        }
        // SAFETY: base is non-null here
        self.base = unsafe { NonNull::new_unchecked(base) };
        Ok(())
    }

    ///bitwise-transfers `v` onto the stack, aligned up from the stack top.
    ///stack storage is POD world — `push` is Copy-gated so nothing leaks.
    pub fn push<T: Copy>(&mut self, v: T) -> Result<S<P>, CapErr> {
        const {
            assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN")
        }
        let size = size_of::<T>();
        let a = align_of::<T>().max(1);
        let start = (self.stack_len() + a - 1) & !(a - 1);
        let end = start + size;
        if end > self.cap() - self.heap_len() {
            return Err(self.cap_err());
        }
        // SAFETY: in-bounds by the check above; bitwise transfer, block performs
        // no drops (stack_shrink contract)
        unsafe {
            ptr::copy_nonoverlapping(
                ptr::from_ref(&v).cast::<u8>(),
                self.data().add(start),
                size,
            );
        }
        self.hdr_mut().stack = end as u32;
        Ok(S(P::from_usize(start), P::from_usize(size)))
    }

    ///pushes `len` × T of uninitialized buffer space at the stack top, T-aligned.
    ///write through `stack_buf_mut` before reading; the block performs no drops.
    pub fn push_buf<T>(&mut self, len: usize) -> Result<S<P>, CapErr> {
        const {
            assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN")
        }
        let size = self.checked_len::<T>(len)?;
        let a = align_of::<T>().max(1);
        let start = (self.stack_len() + a - 1) & !(a - 1);
        let end = start + size;
        if end > self.cap() - self.heap_len() {
            return Err(self.cap_err());
        }
        self.hdr_mut().stack = end as u32;
        Ok(S(P::from_usize(start), P::from_usize(size)))
    }

    ///grows the LAST stack item (its handle end must equal stack_len and its
    ///byte len must be a T multiple) by `len` × T in place — interior items
    ///cannot move. panics on the non-last or mis-sized contract.
    pub fn extend_buf<T>(&mut self, s: S<P>, len: usize) -> Result<S<P>, CapErr> {
        const {
            assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN")
        }
        let size = size_of::<T>();
        assert!(
            s.0.as_usize() + s.1.as_usize() == self.stack_len()
                && (size == 0 || s.1.as_usize().is_multiple_of(size)),
            "extend_buf on non-last or mis-sized item"
        );
        let add = self.checked_len::<T>(len)?;
        let end = s.0.as_usize() + s.1.as_usize() + add;
        if end > self.cap() - self.heap_len() {
            return Err(self.cap_err());
        }
        self.hdr_mut().stack = end as u32;
        Ok(S(s.0, P::from_usize(s.1.as_usize() + add)))
    }

    ///truncates the stack to `pos` bytes; caller guarantees [pos, stack_len)
    ///is dead — no drops performed. the space is reusable by the stack front.
    pub fn stack_shrink(&mut self, pos: usize) {
        assert!(pos <= self.stack_len(), "stack_shrink past bounds");
        //wipe the dead region — the zero-record invariant
        // SAFETY: [pos, stack_len) is inside the stack
        unsafe { ptr::write_bytes(self.data().add(pos), 0, self.stack_len() - pos) };
        self.hdr_mut().stack = pos as u32;
    }

    ///the stack region as raw bytes: `[0, stack_len)`. written contract — the
    ///caller guarantees every claimed byte was written.
    pub fn stack_bytes(&self) -> &[u8] {
        // SAFETY: written contract — callers wrote every stack byte they read
        unsafe { slice::from_raw_parts(self.data(), self.stack_len()) }
    }

    pub fn stack_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: exclusive via &mut self
        unsafe { slice::from_raw_parts_mut(self.data(), self.stack_len()) }
    }

    ///typed view; panics on handle len ≠ size_of::<T>(), misalignment or
    ///out-of-stack. reader contract: the bytes hold a valid `T` (written via
    ///`push` or by the caller).
    pub fn stack<T>(&self, s: S<P>) -> &T {
        let size = size_of::<T>();
        let start = s.0.as_usize();
        assert!(
            s.1.as_usize() == size
                && start.is_multiple_of(align_of::<T>().max(1))
                && start + size <= self.stack_len(),
            "malformed S"
        );
        // SAFETY: start was T-aligned at push; bytes hold a valid T by contract
        unsafe { &*self.data().add(start).cast::<T>() }
    }

    pub fn stack_mut<T>(&mut self, s: S<P>) -> &mut T {
        let size = size_of::<T>();
        let start = s.0.as_usize();
        assert!(
            s.1.as_usize() == size
                && start.is_multiple_of(align_of::<T>().max(1))
                && start + size <= self.stack_len(),
            "malformed S"
        );
        // SAFETY: as stack()
        unsafe { &mut *self.data().add(start).cast::<T>() }
    }

    ///MaybeUninit view over a pushed buffer; count = byte len / T size.
    ///panics on remainder or out-of-stack.
    pub fn stack_buf<T>(&self, s: S<P>) -> &[std::mem::MaybeUninit<T>] {
        let count = self.buf_check::<T>(s.0.as_usize(), s.1.as_usize());
        // SAFETY: MaybeUninit carries no validity requirement
        unsafe { slice::from_raw_parts(self.data().add(s.0.as_usize()).cast(), count) }
    }

    pub fn stack_buf_mut<T>(&mut self, s: S<P>) -> &mut [std::mem::MaybeUninit<T>] {
        let count = self.buf_check::<T>(s.0.as_usize(), s.1.as_usize());
        // SAFETY: exclusive via &mut self
        unsafe { slice::from_raw_parts_mut(self.data().add(s.0.as_usize()).cast(), count) }
    }

    ///allocates `len` × T below the heap bottom: new bottom aligned DOWN, pad
    ///consumed above the region. a fitting freed hole serves instead — best-fit
    ///(smallest fitting len, tie → lowest back_off), O(holes) (frees are rare).
    ///zero-content by the zero-record invariant: regions claim zeroed gap, and
    ///a consumed hole's 8-byte header is zeroed before the span is handed out.
    pub fn alloc<T>(&mut self, len: usize) -> Result<H<P>, CapErr> {
        const {
            assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN")
        }
        let size = self.checked_len::<T>(len)?;
        let a = align_of::<T>().max(1);
        //zero-size needs no space — never splits a hole for it
        if size > 0 && !self.holes.is_empty() && let Some((idx, start)) = self.fit_hole(size, a) {
            return Ok(self.take_hole(idx, start, size));
        }
        let cap = self.cap();
        let free = cap - self.stack_len() - self.heap_len();
        let bottom = cap - self.heap_len();
        if size > free {
            return Err(self.cap_err());
        }
        let pad = (bottom - size) & (a - 1);
        if pad + size > free {
            return Err(self.cap_err());
        }
        let heap = self.heap_len() + pad + size;
        self.hdr_mut().heap = heap as u32;
        Ok(H(P::from_usize(heap), P::from_usize(size)))
    }

    ///best-fit hole scan: smallest fitting len wins, tie → lowest back_off.
    ///start = align_up(cap − off, a); fits when start + size ends in the hole.
    fn fit_hole(&self, size: usize, a: usize) -> Option<(usize, usize)> {
        let cap = self.cap();
        let mut best: Option<(usize, usize)> = None;
        let mut best_len = u32::MAX;
        for (i, &(off, len_h)) in self.holes.iter().enumerate() {
            let span = cap - off as usize;
            let start = (span + a - 1) & !(a - 1);
            //holes are off-sorted, so strict < keeps the lowest off on ties
            if start + size <= span + len_h as usize && len_h < best_len {
                best_len = len_h;
                best = Some((i, start));
            }
        }
        best
    }

    ///consumes hole `idx` for [start, start+size): removes the entry, zeroes
    ///the old header bytes no fresh stamp re-covers, and re-holes ≥ 8-byte
    ///slack (sorted) — heap_len untouched (holes never cross the bottom).
    fn take_hole(&mut self, idx: usize, start: usize, size: usize) -> H<P> {
        let cap = self.cap();
        let (off, len_h) = (self.holes[idx].0 as usize, self.holes[idx].1 as usize);
        let span = cap - off;
        self.holes.remove(idx);
        if len_h >= 8 {
            //old header [span, span+8) is the only debris in the wiped span —
            //zero its bytes landing in the region, the span below, or the span
            //above; stamps below re-cover theirs
            // SAFETY: all three sit inside [span, span+len_h) (header fits it)
            unsafe {
                ptr::write_bytes(self.data().add(span), 0, (start - span).min(8));
                ptr::write_bytes(
                    self.data().add(start),
                    0,
                    (span + 8).saturating_sub(start).min(size),
                );
                ptr::write_bytes(
                    self.data().add(start + size),
                    0,
                    (span + 8).saturating_sub(start + size),
                );
            }
        }
        //slack below is its own hole at the SAME back_off (its start IS cap−off)
        let below = start - span;
        if below >= 8 {
            self.stamp_header(span, below);
            let at = self.holes.partition_point(|e| e.0 < off as u32);
            self.holes.insert(at, (off as u32, below as u32));
        }
        let above = span + len_h - start - size;
        if above >= 8 {
            let back_off = cap - (start + size);
            self.stamp_header(start + size, above);
            let at = self.holes.partition_point(|e| e.0 < back_off as u32);
            self.holes.insert(at, (back_off as u32, above as u32));
        }
        H(P::from_usize(cap - start), P::from_usize(size))
    }

    ///stamps an 8-byte header (MAGIC | len) at a wiped region's start —
    ///hole splits, frees and merged re-stamps all route through here.
    /// SAFETY: caller guarantees the wiped region holds 8 bytes
    fn stamp_header(&mut self, start: usize, len: usize) {
        let mut hdr = [0u8; 8];
        hdr[..4].copy_from_slice(&HOLE_MAGIC.to_le_bytes());
        hdr[4..].copy_from_slice(&(len as u32).to_le_bytes());
        // SAFETY: caller guarantees the stamped region holds 8 bytes —
        // [span, span+8) is inside the freed span and the header's own region
        unsafe {
            ptr::copy_nonoverlapping(hdr.as_ptr(), self.data().add(start), 8);
        }
    }

    ///zeroes an 8-byte header record (coalesce's absorbed-hole cleanup)
    /// SAFETY: caller guarantees [start, start+8) sits inside the data region
    fn erase_stamp(&mut self, start: usize) {
        // SAFETY: the merged span covers it — dead bookkeeping bytes
        unsafe { ptr::write_bytes(self.data().add(start), 0, 8) };
    }

    ///raw heap bytes for `h`; panics on malformed or stale handles (canary:
    ///a live H never names bytes below the current heap bottom). written
    ///contract — the caller guarantees every claimed byte was written.
    pub fn heap(&self, h: H<P>) -> &[u8] {
        let (start, len) = self.heap_bounds(h);
        // SAFETY: written contract — callers wrote the region's bytes
        unsafe { slice::from_raw_parts(self.data().add(start), len) }
    }

    pub fn heap_mut(&mut self, h: H<P>) -> &mut [u8] {
        let (start, len) = self.heap_bounds(h);
        // SAFETY: exclusive via &mut self
        unsafe { slice::from_raw_parts_mut(self.data().add(start), len) }
    }

    ///typed MaybeUninit view; panics on len remainder or misalignment.
    pub fn heap_as<T>(&self, h: H<P>) -> &[std::mem::MaybeUninit<T>] {
        let (start, len) = self.heap_bounds(h);
        let a = align_of::<T>().max(1);
        assert!(start % a == 0 && len % size_of::<T>() == 0, "malformed H for T");
        // SAFETY: MaybeUninit carries no validity requirement
        unsafe { slice::from_raw_parts(self.data().add(start).cast(), len / size_of::<T>()) }
    }

    pub fn heap_as_mut<T>(&mut self, h: H<P>) -> &mut [std::mem::MaybeUninit<T>] {
        let (start, len) = self.heap_bounds(h);
        let a = align_of::<T>().max(1);
        assert!(start % a == 0 && len % size_of::<T>() == 0, "malformed H for T");
        // SAFETY: exclusive via &mut self
        unsafe { slice::from_raw_parts_mut(self.data().add(start).cast(), len / size_of::<T>()) }
    }

    ///rolls the heap bottom back by `len` bytes, wiping the rolled region —
    ///append-only discipline, no freelist; handles naming the rolled region go
    ///stale (the `heap` canaries catch them). panics past empty.
    pub fn pop_heap(&mut self, len: usize) {
        let heap = self.heap_len();
        assert!(len <= heap, "pop_heap past empty");
        //wipe the rolled region — the zero-record invariant
        // SAFETY: [cap − heap, cap − heap + len) is inside the heap
        unsafe { ptr::write_bytes(self.data().add(self.cap() - heap), 0, len) };
        self.hdr_mut().heap = (heap - len) as u32;
        //prune holes below the new bottom — a merged hole straddling the
        //boundary clips to its surviving part above it (at most one: holes
        //are pairwise non-touching)
        let keep = heap - len;
        let cap = self.cap();
        let old = std::mem::take(&mut self.holes);
        self.holes = Vec::with_capacity(old.len());
        for (o, l) in old {
            let (o, l) = (o as usize, l as usize);
            if o <= keep {
                self.holes.push((o as u32, l as u32));
            } else if l > o - keep {
                //survivor's stamp sat in the wiped range (below the new bottom)
                let clipped = l - (o - keep);
                //survivor shorter than a header can't carry one — dropped
                //untracked (its wiped bytes read zero; free space)
                if clipped >= 8 {
                    self.stamp_header(cap - keep, clipped);
                    self.holes.push((keep as u32, clipped as u32));
                }
            }
        }
    }

    ///releases region `h`: wipes the user bytes and stamps an 8-byte hole
    ///header (MAGIC | len) at the region start, recording `(back_off, len)` in
    ///`holes` sorted ascending by back_off, then coalescing touching neighbors
    ///(one merged entry, one merged re-stamp). holes smaller than the header
    ///carry bookkeeping in `holes` only — the header would overrun into the
    ///adjacent live region. a double free panics: the freed region is covered
    ///by any existing hole span (the exact entry or a coalesced superset).
    ///overlapping LIVE-region frees are still undetectable. panics on
    ///malformed or stale handles (the heap() canary).
    pub fn free(&mut self, h: H<P>) {
        let (start, len) = self.heap_bounds(h);
        if len > 0 {
            //double free = any hole whose span covers [start, start+len)
            let cap = self.cap();
            assert!(
                !self.holes.iter().any(|&(o, l)| {
                    let s = cap - o as usize;
                    s <= start && start + len <= s + l as usize
                }),
                "double free"
            );
        }
        //wipe the region — the zero-record invariant
        // SAFETY: in-bounds by heap_bounds
        unsafe { ptr::write_bytes(self.data().add(start), 0, len) };
        let hole = (h.0.as_usize() as u32, h.1.as_usize() as u32);
        let at = self.holes.partition_point(|e| e.0 < hole.0);
        self.holes.insert(at, hole);
        //header needs 8 bytes — smaller holes carry bookkeeping in `holes`
        //only (the stamped header would overrun into the adjacent region)
        if len >= 8 {
            self.stamp_header(start, len);
        }
        self.coalesce(at);
        #[cfg(test)]
        {
            self.frees.push(h);
        }
    }

    ///merges the hole at `idx` with touching neighbors — back_off order maps
    ///monotonically to start address, so only the sorted predecessor (above)
    ///and successor (below) can abut; chained merges re-check each side until
    ///neither touches. absorbed ≥8 holes' stamps are erased; one re-stamp at
    ///the merged start covers the rest.
    fn coalesce(&mut self, mut idx: usize) {
        let cap = self.cap();
        let mut off = self.holes[idx].0 as usize;
        let mut len = self.holes[idx].1 as usize;
        let mut merged_any = false;
        loop {
            let mut merged = false;
            //successor sits below: its end abuts our start when o_s == off+l_s
            if idx + 1 < self.holes.len() {
                let (so, sl) = (self.holes[idx + 1].0 as usize, self.holes[idx + 1].1 as usize);
                if so == off + sl {
                    if len >= 8 {
                        //the old stamp rides strictly inside the merged span
                        self.erase_stamp(cap - off);
                    }
                    off = so;
                    len += sl;
                    self.holes.remove(idx + 1);
                    merged = true;
                }
            }
            //predecessor sits above: its start abuts our end when o_p == off−len
            if idx > 0 {
                let (po, pl) = (self.holes[idx - 1].0 as usize, self.holes[idx - 1].1 as usize);
                if po + len == off {
                    if pl >= 8 {
                        self.erase_stamp(cap - po);
                    }
                    len += pl;
                    self.holes.remove(idx - 1);
                    idx -= 1;
                    merged = true;
                }
            }
            if !merged {
                break;
            }
            merged_any = true;
        }
        self.holes[idx] = (off as u32, len as u32);
        if merged_any && len >= 8 {
            self.stamp_header(cap - off, len);
        }
    }

    ///archive seam: heap allocation + snapshot/rewind only, no stack access.
    pub fn allocator(&mut self) -> Allocator<'_, P> {
        Allocator { block: self }
    }

    pub fn stack_len(&self) -> usize {
        self.hdr().stack as usize
    }

    pub fn heap_len(&self) -> usize {
        self.hdr().heap as usize
    }

    pub fn cap(&self) -> usize {
        self.hdr().cap as usize
    }

    ///bytes free between the fronts.
    pub fn free_bytes(&self) -> usize {
        self.cap() - self.stack_len() - self.heap_len()
    }

    ///regions freed since birth, in order — the release-recursion test's probe.
    #[cfg(test)]
    pub fn freed(&self) -> &[H<P>] {
        &self.frees
    }

    // internal

    #[inline]
    fn hdr(&self) -> &Header {
        // SAFETY: allocation is owned exclusively by self; header never escapes
        unsafe { self.base.cast::<Header>().as_ref() }
    }

    #[inline]
    fn hdr_mut(&mut self) -> &mut Header {
        // SAFETY: exclusive via &mut self; no &Header can be live across it
        unsafe { &mut *self.base.cast::<Header>().as_ptr() }
    }

    #[inline]
    fn data(&self) -> *mut u8 {
        // SAFETY: ptr ≤ HEADER_SIZE + cap, stays inside the allocation
        unsafe { self.base.as_ptr().add(self.hdr().ptr as usize) }
    }

    fn cap_err(&self) -> CapErr {
        CapErr {
            free: self.cap() - self.stack_len() - self.heap_len(),
        }
    }

    fn checked_len<T>(&self, len: usize) -> Result<usize, CapErr> {
        len.checked_mul(size_of::<T>()).ok_or_else(|| self.cap_err())
    }

    fn buf_check<T>(&self, start: usize, byte_len: usize) -> usize {
        assert!(
            start.is_multiple_of(align_of::<T>().max(1)) && start + byte_len <= self.stack_len(),
            "malformed S"
        );
        (size_of::<T>() == 0 || byte_len.is_multiple_of(size_of::<T>())).then_some(byte_len / size_of::<T>())
            .expect("malformed S for T")
    }

    fn heap_bounds(&self, h: H<P>) -> (usize, usize) {
        let h0 = h.0.as_usize();
        let h1 = h.1.as_usize();
        assert!(h0 <= self.heap_len() && h1 <= h0, "malformed or stale H");
        (self.cap() - h0, h1)
    }
}

impl<P> Drop for Block<P> {
    fn drop(&mut self) {
        // SAFETY: base owns exactly this layout from new/grow; header access
        // without the Ptr bound — pure field read
        unsafe {
            let cap = (*self.base.cast::<Header>().as_ptr()).cap as usize;
            dealloc(
                self.base.as_ptr(),
                Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout"),
            )
        }
    }
}

///narrow `&mut` view exposing heap allocation only — the seam a later
///Archive trait consumes instead of a raw byte sink.
pub struct Allocator<'a, P> {
    block: &'a mut Block<P>,
}

impl<'a, P: Ptr> Allocator<'a, P> {
    pub fn alloc<T>(&mut self, len: usize) -> Result<H<P>, CapErr> {
        self.block.alloc::<T>(len)
    }

    ///allocates `data.len()` × T below the heap bottom and memcpys the bytes
    ///in — the archive write primitive (strings, flat vec slices).
    pub fn copy_from<T: Copy>(&mut self, data: &[T]) -> Result<H<P>, CapErr> {
        let h = self.block.alloc::<T>(data.len())?;
        // SAFETY: exclusive via &mut self; h's region fits data exactly —
        // plain Copy bytes, no drops
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr().cast::<u8>(),
                self.block.heap_mut(h).as_mut_ptr(),
                data.len() * size_of::<T>(),
            );
        }
        Ok(h)
    }

    ///writes `v` at slot `i` of region `h` (a T region) — caller guarantees
    ///i < the region's count.
    pub fn write_at<T: Copy>(&mut self, h: H<P>, i: usize, v: T) {
        self.block.heap_as_mut::<T>(h)[i].write(v);
    }

    ///region bytes (read view) — the release/recursion reader; panics on
    ///malformed or stale handles. written contract.
    pub fn bytes(&self, h: H<P>) -> &[u8] {
        self.block.heap(h)
    }

    ///region as T slots (read view) — only archive-written slots may be
    ///assumed init.
    pub fn region<T>(&self, h: H<P>) -> &[std::mem::MaybeUninit<T>] {
        self.block.heap_as::<T>(h)
    }

    ///slot `i` of a T region as initialized — caller guarantees archive wrote
    ///a valid handle there (handles are plain POD).
    pub fn handle<T: Copy>(&self, h: H<P>, i: usize) -> &T {
        // SAFETY: caller contract — archive wrote a valid T at slot i
        unsafe { self.region::<T>(h)[i].assume_init_ref() }
    }

    ///releases the region (Block::free) — Archive::release's path.
    pub fn free(&mut self, h: H<P>) {
        self.block.free(h)
    }

    ///rewind token — heap front + hole bookkeeping (archive's atomicity pair).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            heap: self.block.heap_len(),
            holes: self.block.holes.clone(),
        }
    }

    ///restores the heap front and hole state to `snap` ≤ current: regions
    ///allocated after `snap` go stale — the rolled-back span, every dropped
    ///hole's stamp and every restored hole's span are wiped (zero-record
    ///invariant); handles naming them are the caller's staleness contract.
    pub fn rewind(&mut self, snap: Snapshot) {
        let old_heap = self.block.heap_len();
        let cap = self.block.cap();
        assert!(snap.heap <= old_heap, "rewind ahead of heap front");
        //wipe the dead span the rollback opens
        // SAFETY: [cap − old_heap, cap − snap.heap) is inside the heap
        unsafe {
            ptr::write_bytes(
                self.block.data().add(cap - old_heap),
                0,
                old_heap - snap.heap,
            )
        };
        self.block.hdr_mut().heap = snap.heap as u32;
        //7d: taken hole-allocs wrote archive bytes over restored spans and
        //dropped holes' stamps reach no wipe of their own — clean them all
        let dropped = std::mem::take(&mut self.block.holes);
        for &(o, l) in &dropped {
            if l >= 8 {
                self.block.erase_stamp(cap - o as usize);
            }
        }
        let restored = snap.holes;
        for &(o, l) in &restored {
            let span = cap - o as usize;
            // SAFETY: a restored hole's span sits inside [cap − snap.heap, cap)
            unsafe { ptr::write_bytes(self.block.data().add(span), 0, l as usize) };
            if l >= 8 {
                self.block.stamp_header(span, l as usize);
            }
        }
        self.block.holes = restored;
    }

    ///bytes free between the fronts.
    pub fn free_bytes(&self) -> usize {
        self.block.free_bytes()
    }
}

#[cfg(test)]
mod tests;