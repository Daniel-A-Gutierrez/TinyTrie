//! Arena allocator with stable indices, inline free lists, and block-size reuse.
//!
//! Slots are stored as `Vec<T>` with **zero per-slot overhead**. Freed slots
//! store the next-free index inside their own memory (an untagged union
//! where `T` and `I` share the same bytes). This works because:
//! - All slot types (`[NodeRef; 2]`, `[NodeRef; 4]`, etc.) are ≥ 8 bytes
//! - `I` (u16 or u32) is ≤ 4 bytes
//! - A freed slot's first `size_of::<I>()` bytes hold the next-free pointer
//!
//! **Block-size free lists**: contiguous ranges freed via `free_n` are pushed
//! onto a per-size free list keyed by `n`. When `alloc_n(n, …)` or
//! `alloc_slice` of length `n` is called, the arena first checks the `n`-sized
//! free list for a reusable contiguous block before appending. This makes
//! repeated alloc/free of fixed-size blocks (e.g., Node4, Node16) zero-waste.
//!
//! Individual-slot `alloc`/`free` still use the single-slot free list.
//!
//! `T` must be `Copy` — freed slots are overwritten without drop.
//!
//! `I` is the index type: `u16` for small tries (≤65K slots), `u32` for large.

// ---------------------------------------------------------------------------
// Index trait
// ---------------------------------------------------------------------------

/// A slot index. Must be a trivially-copyable integer that can be
/// converted to/from `usize` for `Vec` indexing.
pub trait Idx: Copy + Eq + std::fmt::Debug {
    fn from_usize(n: usize) -> Self;
    fn to_usize(self) -> usize;
}

impl Idx for u16 {
    #[inline] fn from_usize(n: usize) -> Self { n as u16 }
    #[inline] fn to_usize(self) -> usize { self as usize }
}

impl Idx for u32 {
    #[inline] fn from_usize(n: usize) -> Self { n as u32 }
    #[inline] fn to_usize(self) -> usize { self as usize }
}

// ---------------------------------------------------------------------------
// Arena
// ---------------------------------------------------------------------------

/// A slab-style arena allocator with stable indices and inline free lists.
///
/// `T` is the slot type (e.g., `[NodeRef; 2]` for Node2). Must be `Copy`
/// and at least as large as `I` (so the free-list pointer fits in a freed slot).
/// `I` is the index type (u16 or u32).
///
/// Indices are stable: `alloc` never moves existing slots, and `free`
/// marks slots for reuse without shifting.
///
/// Two free lists:
/// - **Single-slot free list** (`free_head`): for `alloc`/`free` of individual
///   slots. LIFO, stored inline in freed slots.
/// - **Block free-list table** (`block_free`): keyed by block size `n`. When a
///   contiguous range of `n` slots is freed via `free_n`, the start index is
///   pushed onto `block_free[n]`. When `alloc_n(n, …)` or `alloc_slice` of
///   length `n` is called, `block_free[n]` is checked first, yielding
///   zero-waste reuse for fixed-size blocks.
#[derive(Clone)]
pub struct Arena<T: Copy, I: Idx = u32> {
    slots: Vec<T>,
    /// Single-slot free list head (LIFO).
    free_head: Option<I>,
    /// Per-block-size free lists. `block_free[n]` is the head of a linked list
    /// of freed contiguous blocks of length `n`. Each block's first slot stores
    /// the next-block index (or a sentinel if end-of-list).
    block_free: Vec<Option<I>>,
    occupied: usize,
}

impl<T: Copy, I: Idx> Arena<T, I> {
    pub fn new() -> Self {
        Arena {
            slots: Vec::new(),
            free_head: None,
            block_free: Vec::new(),
            occupied: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Arena {
            slots: Vec::with_capacity(capacity),
            free_head: None,
            block_free: Vec::new(),
            occupied: 0,
        }
    }

    /// Ensure `block_free` has at least `n` entries so `block_free[n]` is valid.
    #[inline]
    fn ensure_block_free_len(&mut self, n: usize) {
        if self.block_free.len() <= n {
            self.block_free.resize_with(n + 1, || None);
        }
    }

    /// Allocate a slot, returning its stable index.
    ///
    /// Reuses a freed slot if available (LIFO order), otherwise appends.
    /// Panics if the index overflows `I`.
    pub fn alloc(&mut self, value: T) -> I {
        self.occupied += 1;
        if let Some(idx) = self.free_head {
            let i = idx.to_usize();
            // Read the next-free pointer from the freed slot's memory.
            // Safety: the slot at `idx` was freed, so its first `size_of::<I>()`
            // bytes contain a valid `I` value written by `free()`.
            let next: I = unsafe { self.read_free_ptr(i) };
            self.free_head = if next.to_usize() < self.slots.len() && next != idx {
                Some(next)
            } else {
                None // sentinel: self-referential or out-of-bounds = end of list
            };
            self.slots[i] = value;
            return idx;
        }
        let idx = I::from_usize(self.slots.len());
        assert!(
            idx.to_usize() == self.slots.len(),
            "arena index overflow: slot count exceeds Idx capacity"
        );
        self.slots.push(value);
        idx
    }

    /// Free a slot by index. The slot becomes available for reuse.
    ///
    /// # Panics
    /// Debug builds: panics on double-free or out-of-bounds index.
    ///
    /// # Safety
    /// The caller must ensure `idx` is a valid, currently-occupied slot.
    /// After freeing, the slot's memory is used to store the free-list
    /// pointer — do not read it as `T` until it is reallocated.
    pub fn free(&mut self, idx: I) {
        let i = idx.to_usize();
        debug_assert!(i < self.slots.len(), "free: index out of bounds");
        // Write the current free_head into the freed slot's memory.
        // Safety: we're writing `size_of::<I>()` bytes into a slot that is
        // `size_of::<T>()` bytes, and `size_of::<T>() >= size_of::<I>()`.
        unsafe {
            self.write_free_ptr(i, self.free_head.unwrap_or(idx));
        }
        self.free_head = Some(idx);
        self.occupied -= 1;
    }

    /// Access a slot by index. Caller must ensure the index is valid
    /// (i.e., the slot is currently occupied, not freed).
    #[inline]
    pub fn get(&self, idx: I) -> &T {
        &self.slots[idx.to_usize()]
    }

    /// Access a slot mutably by index. Caller must ensure the index is valid.
    #[inline]
    pub fn get_mut(&mut self, idx: I) -> &mut T {
        &mut self.slots[idx.to_usize()]
    }

    /// Access a contiguous range of `len` slots starting at `start`.
    ///
    /// Returns a slice of `len` elements. Caller must ensure the entire range
    /// `[start, start + len)` is valid (currently occupied slots).
    #[inline]
    pub fn get_range(&self, start: I, len: usize) -> &[T] {
        let s = start.to_usize();
        &self.slots[s..s + len]
    }

    /// Access a contiguous range of `len` slots mutably starting at `start`.
    ///
    /// Returns a mutable slice of `len` elements. Caller must ensure the entire
    /// range `[start, start + len)` is valid (currently occupied slots).
    #[inline]
    pub fn get_range_mut(&mut self, start: I, len: usize) -> &mut [T] {
        let s = start.to_usize();
        &mut self.slots[s..s + len]
    }

    /// Allocate `n` contiguous slots, each initialized with `value`.
    ///
    /// Returns the index of the first slot. Subsequent slots occupy
    /// consecutive indices: `start`, `start+1`, …, `start+n-1`.
    ///
    /// **Reuses freed blocks of the same size first.** When a contiguous range
    /// of `n` slots was previously freed via [`free_n`](Self::free_n), this
    /// method pops from the per-size free list, yielding zero-waste reuse for
    /// fixed-size node types (Node4, Node16, etc.). Falls back to appending
    /// if no free block of size `n` is available.
    ///
    /// # Panics
    /// Panics if `n` is 0 or the resulting index range overflows `I`.
    pub fn alloc_n(&mut self, n: usize, value: T) -> I {
        assert!(n > 0, "alloc_n: cannot allocate 0 slots");

        // Try to reuse a freed block of exactly size `n`.
        if n < self.block_free.len() {
            if let Some(head) = self.block_free[n].take() {
                let i = head.to_usize();
                // Read the next-block pointer from the first slot of the freed block.
                let next: I = unsafe { self.read_free_ptr(i) };
                self.block_free[n] = if next.to_usize() < self.slots.len() && next != head {
                    Some(next)
                } else {
                    None // sentinel or out-of-bounds = end of list
                };
                // Reinitialize all slots in the block.
                for k in 0..n {
                    self.slots[i + k] = value;
                }
                self.occupied += n;
                return head;
            }
        }

        // No reusable block — append.
        let start = self.slots.len();
        let end = start + n;
        let last = I::from_usize(end - 1);
        assert!(
            last.to_usize() == end - 1,
            "alloc_n: index range overflows Idx capacity"
        );
        self.slots.resize(end, value);
        self.occupied += n;
        I::from_usize(start)
    }

    /// Allocate a contiguous slice, copying from `values`.
    ///
    /// Returns the index of the first slot. Each slot `start + i` is
    /// initialized with `values[i]`.
    ///
    /// **Reuses freed blocks of the same size first** — just like
    /// [`alloc_n`](Self::alloc_n), this checks the per-size free list for a
    /// block of exactly `values.len()` slots before appending.
    ///
    /// # Panics
    /// Panics if `values` is empty or the resulting index range overflows `I`.
    pub fn alloc_slice(&mut self, values: &[T]) -> I {
        assert!(!values.is_empty(), "alloc_slice: cannot allocate 0 slots");
        let n = values.len();

        // Try to reuse a freed block of exactly size `n`.
        if n < self.block_free.len() {
            if let Some(head) = self.block_free[n].take() {
                let i = head.to_usize();
                let next: I = unsafe { self.read_free_ptr(i) };
                self.block_free[n] = if next.to_usize() < self.slots.len() && next != head {
                    Some(next)
                } else {
                    None
                };
                // Copy values into the reused block.
                self.slots[i..i + n].copy_from_slice(values);
                self.occupied += n;
                return head;
            }
        }

        // No reusable block — append.
        let start = self.slots.len();
        let end = start + n;
        let last = I::from_usize(end - 1);
        assert!(
            last.to_usize() == end - 1,
            "alloc_slice: index range overflows Idx capacity"
        );
        // extend_from_slice works because T: Copy (which implies Clone)
        self.slots.extend_from_slice(values);
        self.occupied += n;
        I::from_usize(start)
    }

    /// Free a contiguous range of `n` slots starting at `start`.
    ///
    /// The freed block is pushed onto the per-size free list for `n`, so it
    /// can be reused as a whole by [`alloc_n`](Self::alloc_n) or
    /// [`alloc_slice`](Self::alloc_slice) with the same length.
    ///
    /// **Do not mix with individual-slot reuse.** Slots freed via `free_n`
    /// are only reused by `alloc_n`/`alloc_slice` of the same block size.
    /// They are **not** returned by single-slot [`alloc`](Self::alloc).
    ///
    /// # Panics
    /// Debug builds: panics if any slot in the range is out of bounds.
    ///
    /// # Safety
    /// The caller must ensure every slot in `[start, start+n)` is a valid,
    /// currently-occupied slot. After freeing, do not read these slots as `T`
    /// until they are reallocated.
    pub fn free_n(&mut self, start: I, n: usize) {
        assert!(n > 0, "free_n: cannot free 0 slots");
        let s = start.to_usize();
        debug_assert!(
            s + n <= self.slots.len(),
            "free_n: range exceeds arena bounds"
        );

        // Push this block onto the per-size free list for `n`.
        self.ensure_block_free_len(n);
        // Write the current list head into the first slot of the block.
        let head = self.block_free[n];
        unsafe {
            self.write_free_ptr(s, head.unwrap_or(start));
        }
        self.block_free[n] = Some(start);
        self.occupied -= n;
    }

    /// Number of occupied (non-freed) slots.
    pub fn len(&self) -> usize {
        self.occupied
    }

    /// Total slots including freed ones.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    // -----------------------------------------------------------------------
    // Inline free-list pointer I/O
    // -----------------------------------------------------------------------
    //
    // Freed slots store the next-free index in their first `size_of::<I>()`
    // bytes. Since `size_of::<T>() >= size_of::<I>()` (enforced by the
    // caller — NodeRef arrays are always ≥ 8 bytes, I is ≤ 4 bytes),
    // this fits without truncation.

    /// Read the free-list pointer stored in a freed slot.
    ///
    /// # Safety
    /// `i` must be a valid index into `self.slots`, and the slot at `i`
    /// must have been freed (its memory contains a valid `I` value).
    unsafe fn read_free_ptr(&self, i: usize) -> I {
        // SAFETY: caller guarantees `i` is a valid slot index and the slot
        // contains a valid `I` value written by `write_free_ptr`.
        unsafe {
            let slot_ptr = self.slots.as_ptr().add(i) as *const u8;
            let mut val: I = std::mem::zeroed();
            std::ptr::copy_nonoverlapping(
                slot_ptr,
                &mut val as *mut I as *mut u8,
                std::mem::size_of::<I>(),
            );
            val
        }
    }

    /// Write the free-list pointer into a freed slot.
    ///
    /// # Safety
    /// `i` must be a valid index into `self.slots`, and `size_of::<T>()`
    /// must be >= `size_of::<I>()`.
    unsafe fn write_free_ptr(&mut self, i: usize, ptr: I) {
        // SAFETY: caller guarantees `i` is valid and T is large enough for I.
        unsafe {
            let slot_ptr = self.slots.as_mut_ptr().add(i) as *mut u8;
            std::ptr::copy_nonoverlapping(
                &ptr as *const I as *const u8,
                slot_ptr,
                std::mem::size_of::<I>(),
            );
        }
    }
}

impl<T: Copy, I: Idx> Default for Arena<T, I> {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
#[path = "tests/arena.rs"]
mod tests;
