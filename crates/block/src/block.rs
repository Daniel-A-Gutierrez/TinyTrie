//!`Block` — fixed-storage dual-front allocator: header + trailing data in
//!ONE allocation; `[stack | free | heap]` grow toward each other from the
//!data region's ends, every op O(1) arithmetic on two lengths.
//!invariants:
//!- serialized form = the raw allocation bytes; `Header` is exactly 16 bytes,
//!  `ptr` = 16, cap rounded to 16 — fixed-cap blocks are fixed-stride on disk.
//!- the allocation is 16-aligned and `ptr + cap` ≡ 0 mod 16, so BOTH data
//!  ends are absolutely 16-aligned; `align_of::<T>() > MAX_ALIGN` is a
//!  compile panic.
//!- zero-record invariant: every byte of the serialized record is LIVE DATA,
//!  hole bookkeeping, or ZERO — born zeroed (alloc_zeroed), `grow` zeroes the
//!  realloc'd tail (realloc's extension is garbage), reclaims wipe
//!  (`stack_shrink`, `pop_heap`, the archive unwind), `push` zeroes alignment gaps, and
//!  `free` wipes the user bytes leaving a freelist node per hole. no stale
//!  bytes ever reach the record; a deleted item's bytes leave with it.
//!- the freelist lives IN the heap bytes: a `(len, next)` head node occupies
//!  the last NODE bytes of the data region permanently (len = 0 marks the
//!  head); links are back-offsets sorted ascending, `next = 0` ends the
//!  list (safe — no hole can reach offset 0 through the head). every
//!  allocation rounds to ≥ NODE bytes so every freed region can hold its own
//!  node; the list IS the serialized form — a load walks it, nothing is
//!  rebuilt.
//!- the block NEVER drops: stack values transfer bitwise (`push` forgets),
//!  `stack_shrink` reclaims without drops, heap rolls back via `pop_heap`.
//!- space failures return `CapErr { free }` (bytes between the fronts), never
//!  panic; panics are contract violations only (stale handles, misuse).
//!- `grow` reallocs and slides the heap tail (freelist head included) to the
//!  new back: `S` (front-relative) and `H` (back-relative) resolve
//!  identically; every raw slice / `&T` view handed out before the grow is
//!  invalid.
//!- Block is `!Send`/`!Sync` until a threading layer says otherwise.

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc};
use std::fmt;
use std::mem::{MaybeUninit, align_of, size_of};
use std::ptr::{self, NonNull};
use std::slice;

use crate::handle::{H, ReprC, S, ZeroPad};

///max served alignment — both data ends are kept 16-aligned for it.
pub const MAX_ALIGN: usize = 16;

///freelist node: `(len: u16, next: u16)` — 2 × pointer width. the head node
///lives at the data region's back edge; every allocation rounds up to one so
///a freed region always holds its own node.
pub(crate) const NODE: usize = 4;

const HEADER_SIZE: usize = 16;

#[repr(C, align(16))]
struct Header {
    stack: u32,
    heap:  u32,
    cap:   u32,
    ptr:   u32,
}
const _: () = assert!(size_of::<Header>() == HEADER_SIZE);

///space failure; `free` = bytes still between the fronts (max headroom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapErr {
    pub free: usize,
}

impl fmt::Display for CapErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block capacity exceeded ({} bytes free)", self.free)
    }
}

impl std::error::Error for CapErr {}

///`Block` is one allocation plus a lazily-materialized hole cache: the
///in-band freelist (module header) is the durable form — loads pay nothing;
///the first hole-touching op walks it once (bounds-checked, corrupt links
///truncate there) into the `holes` Vec, which then serves every later op.
///the cache is kept in lockstep with the bytes: every mutation rewrites them,
///so serialization reads raw bytes and stays correct.
pub struct Block {
    base:  NonNull<u8>,
    holes: Option<Vec<(u16, u16)>>,
    #[cfg(test)]
    frees: Vec<(u16, u16)>,
}

impl Block {
    ///data region of `cap` bytes (rounded up to 16, the serialized stride).
    ///panics on cap beyond the pointer width. region starts zeroed — the
    ///freelist head (len = 0, next = 0) reads as the empty list.
    pub fn new(cap: usize) -> Self {
        let cap = (cap.max(MAX_ALIGN) + MAX_ALIGN - 1) & !(MAX_ALIGN - 1);
        Self::check_cap(cap);
        let layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: size > 0, align is a power of two; zeroed per the
        // zero-record invariant
        let base = unsafe { alloc_zeroed(layout) };
        if base.is_null() {
            handle_alloc_error(layout);
        }
        // SAFETY: alloc_zeroed returned a live allocation of `layout`
        let mut b = Block {
            // SAFETY: base is non-null here
            base:               unsafe { NonNull::new_unchecked(base) },
            holes:              None,
            #[cfg(test)]
            frees:              Vec::new(),
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
    ///+ cap, cap a 16-multiple inside the pointer width. the freelist rides
    ///the record's own bytes and loads LAZILY — nothing is walked here; the
    ///first hole-touching op materializes the cache (bounds-checked, corrupt
    ///links truncate there), so a malformed record can never read out of
    ///bounds but a load is a copy.
    pub(crate) fn from_record(record: &[u8]) -> Self {
        let cap = u32::from_le_bytes([record[8], record[9], record[10], record[11]]) as usize;
        assert!(record.len() == HEADER_SIZE + cap, "record size != 16 + cap");
        Self::check_cap(cap);
        let layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: size > 0, align a power of two
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            handle_alloc_error(layout);
        }
        // SAFETY: fresh allocation — the record copies over header and data
        // region intact (zero-record invariant carried by the record itself)
        unsafe {
            ptr::copy_nonoverlapping(record.as_ptr(), base, record.len());
        }
        // SAFETY: base is non-null here
        Block {
            // SAFETY: as above
            base:               unsafe { NonNull::new_unchecked(base) },
            holes:              None,
            #[cfg(test)]
            frees:              Vec::new(),
        }
    }

    fn check_cap(cap: usize) {
        assert!(cap <= u16::MAX as usize, "cap exceeds pointer width");
    }

    ///grows to `new_cap` data bytes (rounded up to 16). ≤ cap is a no-op when
    ///the used bytes still fit, else an error. realloc + heap-tail slide (the
    ///freelist head rides the tail's last NODE bytes): all `S`/`H` resolve
    ///identically, all previously handed-out raw slices and `&T` views die.
    ///panic-free on space: too-small → `CapErr`.
    pub fn grow(&mut self, new_cap: usize) -> Result<(), CapErr> {
        let used = self.stack_len() + self.heap_len();
        //checked rounding — a near-usize::MAX ask must not wrap small and
        //read as a no-op grow
        let new_cap = new_cap
            .checked_add(MAX_ALIGN - 1)
            .map(|v| v & !(MAX_ALIGN - 1))
            .expect("cap exceeds pointer width");
        let cap = self.cap();
        if new_cap <= cap {
            return if new_cap < used { Err(self.cap_err()) } else { Ok(()) };
        }
        Self::check_cap(new_cap);
        let old_layout = Layout::from_size_align(HEADER_SIZE + cap, MAX_ALIGN).expect("layout");
        // SAFETY: realloc of the owned block; on move, bytes persist — S (front-
        // relative) and H (back-relative) re-resolve unchanged, header rides along
        let base = unsafe { realloc(self.base.as_ptr(), old_layout, HEADER_SIZE + new_cap) };
        if base.is_null() {
            let layout =
                Layout::from_size_align(HEADER_SIZE + new_cap, MAX_ALIGN).expect("layout");
            handle_alloc_error(layout);
        }
        // SAFETY: realloc's extension is uninitialized — zero the new tail so
        // the zero-record invariant holds (the slide below may overlap it)
        unsafe { ptr::write_bytes(base.add(HEADER_SIZE + cap), 0, new_cap - cap) };
        // SAFETY: same invariants as hdr_mut; allocation moved under us
        let h = unsafe { &mut *base.cast::<Header>() };
        h.cap = new_cap as u32;
        // heap tail + freelist head sit at old offsets: slide them back by
        // the grown amount (back-relative offs are invariant under the slide)
        let heap = h.heap as usize;
        // SAFETY: both spans inside the new allocation (new_cap > cap);
        // ptr::copy is memmove — source and destination overlap when heap > 0
        unsafe {
            ptr::copy(
                base.add(HEADER_SIZE + cap - NODE - heap),
                base.add(HEADER_SIZE + new_cap - NODE - heap),
                NODE + heap,
            );
        }
        // SAFETY: base is non-null here
        self.base = unsafe { NonNull::new_unchecked(base) };
        Ok(())
    }

    ///pushes `v` onto the stack front, bitwise. the block never runs drops
    ///and never interprets `v`'s bytes — it memcpy's `size_of::<T>()` of them,
    ///which is exactly why the bounds: `Copy` keeps reclaimed bytes from ever
    ///running drop glue, `ZeroPad` promises every byte of `v` (padding
    ///included) is defined so the memcpy cannot carry uninitialized bytes
    ///into a record, and `ReprC` pins the field order the record shows.
    ///alignment gap bytes below `v` are zeroed (usually zero-length).
    pub fn push<T: Copy + ZeroPad + ReprC>(&mut self, v: T) -> Result<S<T>, CapErr> {
        const { assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN") }
        let size = size_of::<T>();
        let a = align_of::<T>().max(1);
        let start = (self.stack_len() + a - 1) & !(a - 1);
        let end = start + size;
        if end > self.free_floor() - self.heap_len() {
            return Err(self.cap_err());
        }
        if start > self.stack_len() {
            // SAFETY: [stack_len, start) is inside the stack region
            unsafe {
                ptr::write_bytes(self.data().add(self.stack_len()), 0, start - self.stack_len())
            };
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
        Ok(S::new(start as u16, size as u16))
    }

    ///pushes `len` × T of uninitialized buffer space at the stack top,
    ///T-aligned. write through `stack_buf_mut` before reading; the block
    ///performs no drops. the alignment gap is zeroed.
    pub fn push_buf<T>(&mut self, len: usize) -> Result<S<T>, CapErr> {
        const { assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN") }
        let size = self.checked_len::<T>(len)?;
        let a = align_of::<T>().max(1);
        let start = (self.stack_len() + a - 1) & !(a - 1);
        let end = start + size;
        if end > self.free_floor() - self.heap_len() {
            return Err(self.cap_err());
        }
        if start > self.stack_len() {
            // SAFETY: [stack_len, start) is inside the stack region
            unsafe {
                ptr::write_bytes(self.data().add(self.stack_len()), 0, start - self.stack_len())
            };
        }
        self.hdr_mut().stack = end as u32;
        Ok(S::new(start as u16, size as u16))
    }

    ///grows the LAST stack item (its handle end must equal stack_len and its
    ///byte len must be a T multiple) by `len` × T in place — interior items
    ///cannot move. panics on the non-last or mis-sized contract.
    pub fn extend_buf<T>(&mut self, s: S<T>, len: usize) -> Result<S<T>, CapErr> {
        const { assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN") }
        let size = size_of::<T>();
        assert!(
            s.off as usize + s.len as usize == self.stack_len()
                && (size == 0 || (s.len as usize).is_multiple_of(size)),
            "extend_buf on non-last or mis-sized item"
        );
        let add = self.checked_len::<T>(len)?;
        let end = s.off as usize + s.len as usize + add;
        if end > self.free_floor() - self.heap_len() {
            return Err(self.cap_err());
        }
        self.hdr_mut().stack = end as u32;
        Ok(S::new(s.off, (s.len as usize + add) as u16))
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

    ///typed view of a pushed item; panics only on stale handles (a live `S<T>`
    ///cannot be malformed — Block constructs them T-aligned).
    pub fn stack<T>(&self, s: S<T>) -> &T {
        let start = s.off as usize;
        assert!(start + size_of::<T>() <= self.stack_len(), "stale S");
        // SAFETY: start was T-aligned at push; bytes hold a valid T by contract
        unsafe { &*self.data().add(start).cast::<T>() }
    }

    pub fn stack_mut<T>(&mut self, s: S<T>) -> &mut T {
        let start = s.off as usize;
        assert!(start + size_of::<T>() <= self.stack_len(), "stale S");
        // SAFETY: as stack()
        unsafe { &mut *self.data().add(start).cast::<T>() }
    }

    ///MaybeUninit view over a pushed buffer; count = byte len / T size.
    ///panics on remainder or out-of-stack.
    pub fn stack_buf<T>(&self, s: S<T>) -> &[MaybeUninit<T>] {
        let count = self.buf_check::<T>(s.off as usize, s.len as usize);
        // SAFETY: MaybeUninit carries no validity requirement
        unsafe { slice::from_raw_parts(self.data().add(s.off as usize).cast(), count) }
    }

    pub fn stack_buf_mut<T>(&mut self, s: S<T>) -> &mut [MaybeUninit<T>] {
        let count = self.buf_check::<T>(s.off as usize, s.len as usize);
        // SAFETY: exclusive via &mut self
        unsafe { slice::from_raw_parts_mut(self.data().add(s.off as usize).cast(), count) }
    }

    ///allocates `len` × T below the heap bottom: new bottom aligned DOWN, pad
    ///consumed above the region. a fitting freed hole serves instead —
    /// best-fit (smallest fitting hole, tie → lowest back-offset), O(holes)
    /// (frees are rare). the allocated span rounds up to NODE and to a
    /// multiple of the T size, so `H::len` may exceed the requested bytes
    /// (the tail reads zero) and every freed region can hold a freelist node.
    ///zero-content by the zero-record invariant: regions claim zeroed gap,
    ///and a consumed hole's node bytes are zeroed before the span is handed
    ///out.
    pub fn alloc<T>(&mut self, len: usize) -> Result<H<T>, CapErr> {
        const { assert!(align_of::<T>() <= MAX_ALIGN, "T align exceeds MAX_ALIGN") }
        let size = self.checked_len::<T>(len)?;
        if size == 0 {
            return Ok(H::new(0, 0));
        }
        let a = align_of::<T>().max(1);
        let span = size.max(NODE).div_ceil(size) * size;
        if self.head_next() != 0
            && let Some((i, start)) = self.fit_hole(span, a)
        {
            let cap = self.cap();
            let (h_off, h_len) = self.holes_mut()[i];
            let hole_start = cap - h_off as usize;
            let hole_end = hole_start + h_len as usize;
            let above = start - hole_start;
            let below = hole_end - (start + span);
            //absorbed align slack: the taken hole's node bytes — ZEROED
            //(stale bookkeeping must not reach the record)
            if above > 0 && above < NODE {
                // SAFETY: inside the taken hole's span
                unsafe { ptr::write_bytes(self.data().add(hole_start), 0, above) };
            }
            //the taken hole is replaced by its surviving slacks (off-sorted;
            //sub-NODE slacks are dead — the above one zeroed, the below one
            //hole interior, already zero)
            let mut repl: Vec<(u16, u16)> = Vec::new();
            if above >= NODE {
                repl.push((h_off, above as u16));
            }
            if below >= NODE {
                repl.push(((cap - (start + span)) as u16, below as u16));
            }
            self.holes_mut().splice(i..=i, repl);
            self.sync_holes();
            //zero the served span — the taken hole's node bytes are stale
            //bookkeeping and must not reach the caller's record
            // SAFETY: [start, start + span) is inside the data region
            unsafe { ptr::write_bytes(self.data().add(start), 0, span) };
            return Ok(H::new((cap - start) as u16, span as u16));
        }
        let free = self.free_bytes();
        let bottom = self.free_floor() - self.heap_len();
        if span > free {
            return Err(self.cap_err());
        }
        let pad = (bottom - span) & (a - 1);
        if pad + span > free {
            return Err(self.cap_err());
        }
        let heap = self.heap_len() + pad + span;
        self.hdr_mut().heap = heap as u32;
        Ok(H::new((NODE + heap) as u16, span as u16))
    }

    ///best-fit hole scan: smallest fitting hole wins, tie → lowest off.
    ///start = align_up(hole start address, a); fits when [start, start+span)
    ///ends inside the hole. returns the chosen index and the aligned start.
    fn fit_hole(&mut self, span: usize, a: usize) -> Option<(usize, usize)> {
        let cap = self.cap();
        let holes = self.holes_mut();
        let mut best: Option<(usize, usize)> = None;
        let mut best_len = u32::MAX;
        for (i, &(off, len_h)) in holes.iter().enumerate() {
            let hole_start = cap - off as usize;
            let start = (hole_start + a - 1) & !(a - 1);
            //holes are off-sorted, so strict < keeps the lowest off on ties
            if start + span <= hole_start + len_h as usize && (len_h as u32) < best_len {
                best_len = len_h as u32;
                best = Some((i, start));
            }
        }
        best
    }

    ///releases region `h`: wipes the user bytes and threads the region into
    ///the freelist — its own `(len, next)` node at the region start, list
    ///kept off-sorted ascending, neighbors coalesced when touching OR
    ///separated by an unallocatable dead gap (< NODE — every live region is
    ///≥ NODE, so a sub-NODE gap can only be dead align slack; the join makes
    ///hole-take unwinding exact). a double free panics: the freed region is
    ///covered by any existing hole span (the exact entry or a coalesced
    ///superset). overlapping LIVE-region frees are still undetectable.
    ///panics on stale handles (the heap_bounds canary).
    pub fn free<T>(&mut self, h: H<T>) {
        let (start, len) = self.heap_bounds(&h);
        if len == 0 {
            return;
        }
        let cap = self.cap();
        let off = (cap - start) as u16;
        let holes = self.holes_mut();
        //double free = any hole whose span covers [start, start+len)
        assert!(
            !holes.iter().any(|&(o, l)| {
                (o as usize) >= off as usize && o as usize + len <= off as usize + l as usize
            }),
            "double free"
        );
        //binary insert (off-sorted)
        let at = holes.partition_point(|e| e.0 < off);
        holes.insert(at, (off, len as u16));
        //coalesce fold: for consecutive off-sorted holes A (above, smaller
        //off) and B, the address gap is B.off − B.len − A.off — join when it
        //is < NODE (0 = touching, 1..NODE−1 = dead align crumbs)
        let mut merged: Vec<(u16, u16)> = Vec::with_capacity(holes.len());
        for h in holes.drain(..) {
            if let Some(last) = merged.last_mut() {
                let gap = h.0 as usize - h.1 as usize - last.0 as usize;
                if gap < NODE {
                    last.0 = h.0;
                    last.1 += h.1 + gap as u16;
                    continue;
                }
            }
            merged.push(h);
        }
        *self.holes.as_mut().unwrap() = merged;
        self.sync_holes();
        #[cfg(test)]
        {
            self.frees.push((off, len as u16));
        }
    }

    ///rolls the heap bottom back by `len` bytes, wiping the rolled region —
    ///append-only discipline; handles naming the rolled region go stale (the
    ///`heap` canaries catch them). holes fully below the new bottom die; a
    ///straddling hole clips to its surviving part (dropped when too small for
    ///a node — a dead gap). panics past empty.
    pub fn pop_heap(&mut self, len: usize) {
        let heap = self.heap_len();
        assert!(len <= heap, "pop_heap past empty");
        let floor = (NODE + heap - len) as u16;
        //collect BEFORE the wipe — a straddling hole's node sits inside the
        //rolled span and would read as a zeroed (dead) link after it
        let holes = std::mem::take(self.holes_mut());
        let mut kept: Vec<(u16, u16)> = Vec::new();
        for (off, l) in holes {
            if off <= floor {
                //entirely above the new bottom
                kept.push((off, l));
            } else if l > off - floor {
                //straddler: survivor [new bottom, hole top), clipped
                let clipped = l - (off - floor);
                if clipped as usize >= NODE {
                    kept.push((floor, clipped));
                }
            }
            //else: entirely below — dead with the rolled region
        }
        //wipe the rolled region — the zero-record invariant
        // SAFETY: [old bottom, old bottom + len) is inside the heap
        unsafe { ptr::write_bytes(self.data().add(self.free_floor() - heap), 0, len) };
        self.hdr_mut().heap = (heap - len) as u32;
        *self.holes.as_mut().unwrap() = kept;
        self.sync_holes();
    }

    ///materializes the hole cache: one bounds-checked walk of the in-band
    ///list — the load-time validation, paid at first use. corrupt links
    ///truncate the list in the bytes (records are trusted for data, never
    ///for pointer safety).
    fn holes_mut(&mut self) -> &mut Vec<(u16, u16)> {
        if self.holes.is_none() {
            let cap = self.cap();
            let mut out = Vec::new();
            let mut prev_addr = cap - NODE;
            let mut prev_len = 0u16;
            let mut next = self.node_at(prev_addr).1;
            let mut hops = 0usize;
            while next != 0 {
                let off = next as usize;
                //real holes sit strictly past the head (off ≥ NODE + len);
                //the head's own address (off == NODE) is a self-link
                if off <= NODE || off > cap || hops > cap / 2 {
                    self.write_node(prev_addr, prev_len, 0);
                    break;
                }
                let (len, further) = self.node_at(cap - off);
                //a real hole satisfies off ≥ NODE + len — anything else (a
                //garbage node aliasing the head included) truncates the walk
                if len as usize + NODE > off {
                    self.write_node(prev_addr, prev_len, 0);
                    break;
                }
                out.push((next, len));
                prev_addr = cap - off;
                prev_len = len;
                next = further;
                hops += 1;
            }
            self.holes = Some(out);
        }
        self.holes.as_mut().unwrap()
    }

    ///rewrites the whole in-band freelist from the cache: each hole span is
    ///zeroed then its node written, head last — hole bytes are zero or live
    ///bookkeeping only (zero-record law), and absorbed nodes never survive.
    ///the cache stays in lockstep with the bytes, so raw reads (records,
    ///heap_span) are always current.
    fn sync_holes(&mut self) {
        //taken out for the write pass — the borrow must not overlap the
        //node writes
        let holes = std::mem::take(self.holes.as_mut().unwrap());
        debug_assert!(holes.is_sorted_by(|a, b| a.0 <= b.0), "freelist must be off-sorted");
        let cap = self.cap();
        for &(off, len) in &holes {
            let start = cap - off as usize;
            // SAFETY: [start, start + len) is inside the data region
            unsafe { ptr::write_bytes(self.data().add(start), 0, len as usize) };
        }
        for (i, &(off, len)) in holes.iter().enumerate() {
            let next = holes.get(i + 1).map_or(0, |h| h.0);
            self.write_node(cap - off as usize, len, next);
        }
        let first = holes.first().map_or(0, |h| h.0);
        self.write_node(cap - NODE, 0, first);
        self.holes = Some(holes);
    }

    ///the head node's `next` link — 0 when the list is empty.
    fn head_next(&self) -> u16 {
        self.node_at(self.cap() - NODE).1
    }

    ///reads the `(len, next)` node at data-region byte `addr`.
    /// SAFETY: caller guarantees [addr, addr + NODE) inside the data region —
    /// runtime lists are built by set_holes, loads walk validated.
    fn node_at(&self, addr: usize) -> (u16, u16) {
        // SAFETY: caller guarantee above
        let p = unsafe { self.data().add(addr) };
        // SAFETY: caller guarantee above
        unsafe {
            (
                u16::from_le_bytes(ptr::read(p.cast())),
                u16::from_le_bytes(ptr::read(p.add(2).cast())),
            )
        }
    }

    ///writes the `(len, next)` node at data-region byte `addr`.
    /// SAFETY: caller guarantees [addr, addr + NODE) inside the data region.
    fn write_node(&mut self, addr: usize, len: u16, next: u16) {
        // SAFETY: caller guarantee above
        let p = unsafe { self.data().add(addr) };
        // SAFETY: caller guarantee above
        unsafe {
            ptr::copy_nonoverlapping(len.to_le_bytes().as_ptr() as *const u8, p, 2);
            ptr::copy_nonoverlapping(next.to_le_bytes().as_ptr() as *const u8, p.add(2), 2);
        }
    }

    ///archive seam: heap allocation + release/rollback only, no stack access.
    pub fn allocator(&mut self) -> Allocator<'_> {
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

    ///bytes free between the fronts (the freelist head is not free space).
    pub fn free_bytes(&self) -> usize {
        self.free_floor() - self.stack_len() - self.heap_len()
    }

    ///the allocation area's back edge — cap minus the freelist head.
    fn free_floor(&self) -> usize {
        self.cap() - NODE
    }

    ///the whole heap tail including the freelist head —
    ///`[cap − NODE − heap_len, cap)`; the serialized record's last stretch.
    pub fn heap_span(&self) -> &[u8] {
        let start = self.cap() - NODE - self.heap_len();
        // SAFETY: inside the data region by the heap front
        unsafe { slice::from_raw_parts(self.data().add(start), NODE + self.heap_len()) }
    }

    ///typed read of a heap region; count = byte len / T size. panics on
    ///malformed or stale handles (the canary) — alignment is guaranteed by
    ///the unforgeable `H<T>`. written contract — the caller guarantees every
    ///claimed element was written.
    pub fn heap<T>(&self, h: H<T>) -> &[T] {
        let (start, len) = self.heap_bounds(&h);
        let size = size_of::<T>();
        if size == 0 {
            return &[];
        }
        // SAFETY: written contract — callers wrote the region's bytes
        unsafe { slice::from_raw_parts(self.data().add(start).cast(), len / size) }
    }

    ///region as T slots — uninitialized view; only archive-written slots may
    ///be assumed init.
    pub fn heap_uninit<T>(&self, h: H<T>) -> &[MaybeUninit<T>] {
        let (start, len) = self.heap_bounds(&h);
        let size = size_of::<T>();
        if size == 0 {
            return &[];
        }
        // SAFETY: MaybeUninit carries no validity requirement
        unsafe { slice::from_raw_parts(self.data().add(start).cast(), len / size) }
    }

    ///mutable uninit view over a heap region — write through it before any
    ///read; the block performs no drops.
    pub fn heap_uninit_mut<T>(&mut self, h: H<T>) -> &mut [MaybeUninit<T>] {
        let (start, len) = self.heap_bounds(&h);
        let size = size_of::<T>();
        if size == 0 {
            return &mut [];
        }
        // SAFETY: exclusive via &mut self; MaybeUninit carries no validity
        // requirement
        unsafe { slice::from_raw_parts_mut(self.data().add(start).cast(), len / size) }
    }

    ///regions freed since birth, as `(back-offset, byte-len)` pairs — the
    ///release-recursion test's probe.
    #[cfg(test)]
    pub fn freed(&self) -> &[(u16, u16)] {
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
        CapErr { free: self.free_bytes() }
    }

    fn checked_len<T>(&self, len: usize) -> Result<usize, CapErr> {
        len.checked_mul(size_of::<T>()).ok_or_else(|| self.cap_err())
    }

    fn buf_check<T>(&self, start: usize, byte_len: usize) -> usize {
        assert!(
            start.is_multiple_of(align_of::<T>().max(1))
                && start + byte_len <= self.stack_len(),
            "stale S"
        );
        (size_of::<T>() == 0 || byte_len.is_multiple_of(size_of::<T>()))
            .then_some(if size_of::<T>() == 0 { 0 } else { byte_len / size_of::<T>() })
            .expect("malformed S for T")
    }

    ///(start address, byte len) for a region; the canary for malformed or
    ///stale handles — a live H never names bytes below the current heap
    ///bottom.
    fn heap_bounds<T>(&self, h: &H<T>) -> (usize, usize) {
        let (off, len) = (h.off as usize, h.len as usize);
        assert!(off <= NODE + self.heap_len() && len <= off, "malformed or stale H");
        (self.cap() - off, len)
    }
}

impl Drop for Block {
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
pub struct Allocator<'a> {
    block: &'a mut Block,
}

impl Allocator<'_> {
    pub fn alloc<T>(&mut self, len: usize) -> Result<H<T>, CapErr> {
        self.block.alloc::<T>(len)
    }

    ///allocates `data.len()` × T below the heap bottom and memcpys the bytes
    ///in — the raw slice archive primitive (strings, flat vec elements).
    pub fn alloc_slice<T: Copy>(&mut self, data: &[T]) -> Result<H<T>, CapErr> {
        let h = self.block.alloc::<T>(data.len())?;
        let bytes = data.len() * size_of::<T>();
        // SAFETY: exclusive via &mut self; the region fits data exactly —
        // plain Copy bytes, no drops; the tail of a NODE-rounded region
        // reads zero (zero-record invariant)
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr().cast::<u8>(),
                self.block.data().add(self.block.cap() - h.off as usize),
                bytes,
            );
        }
        Ok(h)
    }

    ///slot `i` of a T region as initialized — caller guarantees archive wrote
    ///a valid T there (handles are plain POD).
    pub fn slot<T: Copy>(&self, h: H<T>, i: usize) -> &T {
        // SAFETY: caller contract — archive wrote a valid T at slot i
        unsafe { self.block.heap_uninit(h)[i].assume_init_ref() }
    }

    ///slot write into an uninitialized region — the archive-fill primitive.
    ///caller guarantees i < the region's count.
    pub(crate) fn write_slot<T: Copy>(&mut self, h: H<T>, i: usize, v: T) {
        let (start, len) = self.block.heap_bounds(&h);
        debug_assert!((i + 1) * size_of::<T>() <= len, "slot write out of region");
        // SAFETY: inside the region by the debug bound above; the region is
        // uninit archive scratch
        unsafe {
            ptr::copy_nonoverlapping(
                ptr::from_ref(&v).cast::<u8>(),
                self.block.data().add(start + i * size_of::<T>()),
                size_of::<T>(),
            );
        }
    }

    ///releases the region (Block::free) — Archive::release's path.
    pub fn free<T>(&mut self, h: H<T>) {
        self.block.free(h)
    }

    ///the heap front — the unwind anchor for multi-allocation archives (see
    ///rollback).
    pub fn heap_len(&self) -> usize {
        self.block.heap_len()
    }

    ///rolls the heap front back to a pre-archive position, wiping the rolled
    ///span — the second half of the unwind protocol (release children first,
    ///then this); everything freshly allocated below the entry front dies
    ///wholesale, pre-existing holes ride through untouched.
    pub fn rollback(&mut self, heap_at_entry: usize) {
        self.block.pop_heap(self.block.heap_len() - heap_at_entry);
    }

    ///bytes free between the fronts.
    pub fn free_bytes(&self) -> usize {
        self.block.free_bytes()
    }
}

#[cfg(test)]
mod tests;
