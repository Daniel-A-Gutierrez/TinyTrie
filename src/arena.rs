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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_get() {
        let mut arena: Arena<[u32; 2], u16> = Arena::new();
        let a = arena.alloc([10, 20]);
        let b = arena.alloc([30, 40]);
        let c = arena.alloc([50, 60]);
        assert_eq!(*arena.get(a), [10, 20]);
        assert_eq!(*arena.get(b), [30, 40]);
        assert_eq!(*arena.get(c), [50, 60]);
        assert_eq!(arena.len(), 3);
    }

    #[test]
    fn free_and_reuse() {
        let mut arena: Arena<[u32; 2], u16> = Arena::new();
        let _a = arena.alloc([10, 20]);
        let b = arena.alloc([30, 40]);
        let _c = arena.alloc([50, 60]);
        arena.free(b);
        assert_eq!(arena.len(), 2);

        let d = arena.alloc([99, 88]); // reuses b
        assert_eq!(d, b);
        assert_eq!(*arena.get(d), [99, 88]);
        assert_eq!(arena.len(), 3);
    }

    #[test]
    fn stable_indices() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let a = arena.alloc([1, 2]);
        let b = arena.alloc([3, 4]);
        let _c = arena.alloc([5, 6]);
        arena.free(b);
        // a is still valid — freeing b doesn't shift anything
        assert_eq!(*arena.get(a), [1, 2]);
    }

    #[test]
    fn free_chain() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let a = arena.alloc([10, 0]);
        let _b = arena.alloc([20, 0]);
        let c = arena.alloc([30, 0]);
        arena.free(a);
        arena.free(c);
        // Free list: c -> a (LIFO)
        let d = arena.alloc([100, 0]); // reuses c
        let e = arena.alloc([200, 0]); // reuses a
        assert_eq!(d, c);
        assert_eq!(e, a);
        assert_eq!(*arena.get(d), [100, 0]);
        assert_eq!(*arena.get(e), [200, 0]);
        assert_eq!(arena.len(), 3); // _b, d, e
    }

    #[test]
    fn u16_index_type() {
        let mut arena: Arena<[u32; 2], u16> = Arena::new();
        let idx = arena.alloc([42, 0]);
        assert_eq!(idx.to_usize(), 0);
        assert_eq!(*arena.get(idx), [42, 0]);
    }

    #[test]
    fn with_capacity() {
        let mut arena: Arena<[u32; 2], u32> = Arena::with_capacity(100);
        for i in 0..100u32 {
            arena.alloc([i, i + 1]);
        }
        assert_eq!(arena.len(), 100);
    }

    #[test]
    fn alloc_n_basic() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let start = arena.alloc_n(4, [0xFF, 0xAA]);
        assert_eq!(start.to_usize(), 0);
        for i in 0..4 {
            assert_eq!(*arena.get(u32::from_usize(i)), [0xFF, 0xAA]);
        }
        assert_eq!(arena.len(), 4);
        assert_eq!(arena.capacity(), 4);
    }

    #[test]
    fn alloc_n_after_singles() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let a = arena.alloc([1, 2]);  // index 0
        let b = arena.alloc([3, 4]);  // index 1
        let start = arena.alloc_n(3, [9, 9]); // indices 2, 3, 4
        assert_eq!(a.to_usize(), 0);
        assert_eq!(b.to_usize(), 1);
        assert_eq!(start.to_usize(), 2);
        assert_eq!(*arena.get(a), [1, 2]);
        assert_eq!(*arena.get(b), [3, 4]);
        for i in 2..=4 {
            assert_eq!(*arena.get(u32::from_usize(i)), [9, 9]);
        }
        assert_eq!(arena.len(), 5);
    }

    #[test]
    fn alloc_n_u16_overflow() {
        let mut arena: Arena<[u32; 2], u16> = Arena::new();
        // Fill up to near u16 max
        for _ in 0..65534 {
            arena.alloc([0, 0]);
        }
        assert_eq!(arena.len(), 65534);
        // Can still alloc_n a small range
        let s = arena.alloc_n(2, [1, 1]); // indices 65534, 65535
        assert_eq!(s.to_usize(), 65534);
        assert_eq!(arena.len(), 65536);
        // Next alloc_n should panic (overflow)
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut a: Arena<[u32; 2], u16> = Arena::new();
            for _ in 0..65535 {
                a.alloc([0, 0]);
            }
            a.alloc_n(2, [0, 0]); // would need index 65536, which overflows u16
        }));
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_basic() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let values: Vec<[u32; 2]> = (0..5).map(|i| [i, i * 10]).collect();
        let start = arena.alloc_slice(&values);
        assert_eq!(start.to_usize(), 0);
        for i in 0..5 {
            assert_eq!(*arena.get(u32::from_usize(i)), [i as u32, i as u32 * 10]);
        }
        assert_eq!(arena.len(), 5);
    }

    #[test]
    fn alloc_slice_after_alloc() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let first = arena.alloc([100, 200]);
        let values: Vec<[u32; 2]> = (0..3).map(|i| [i, i + 1]).collect();
        let start = arena.alloc_slice(&values);
        assert_eq!(first.to_usize(), 0);
        assert_eq!(start.to_usize(), 1);
        assert_eq!(*arena.get(first), [100, 200]);
        for i in 0..3 {
            assert_eq!(*arena.get(u32::from_usize(1 + i)), [i as u32, i as u32 + 1]);
        }
    }

    #[test]
    fn free_n_basic() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let start = arena.alloc_n(4, [42, 0]);
        let single = arena.alloc([99, 88]);
        assert_eq!(arena.len(), 5);

        arena.free_n(start, 4);
        assert_eq!(arena.len(), 1);
        // single allocation is still valid
        assert_eq!(*arena.get(single), [99, 88]);
    }

    #[test]
    fn free_n_then_alloc_n_reuse() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        // Allocate a block of 3, then free it.
        let block = arena.alloc_n(3, [0, 0]); // indices 0, 1, 2
        assert_eq!(arena.len(), 3);
        assert_eq!(arena.capacity(), 3);

        arena.free_n(block, 3);
        assert_eq!(arena.len(), 0);
        assert_eq!(arena.capacity(), 3); // slots still present

        // alloc_n(3, …) should reuse the freed block at index 0.
        let reused = arena.alloc_n(3, [7, 7]);
        assert_eq!(reused, block); // same start index
        for i in 0..3 {
            assert_eq!(*arena.get(u32::from_usize(i)), [7, 7]);
        }
        assert_eq!(arena.len(), 3);
        assert_eq!(arena.capacity(), 3); // didn't grow
    }

    #[test]
    fn free_n_then_alloc_slice_reuse() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let values: Vec<[u32; 2]> = (0..4).map(|i| [i, i * 10]).collect();
        let block = arena.alloc_slice(&values);
        arena.free_n(block, 4);

        // alloc_slice of same size reuses the block.
        let new_vals: Vec<[u32; 2]> = (0..4).map(|i| [i * 100, i]).collect();
        let reused = arena.alloc_slice(&new_vals);
        assert_eq!(reused, block);
        for i in 0..4 {
            assert_eq!(*arena.get(u32::from_usize(i)), [i as u32 * 100, i as u32]);
        }
        assert_eq!(arena.capacity(), 4); // didn't grow
    }

    #[test]
    fn free_n_different_sizes_no_cross_reuse() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        // Allocate blocks of size 3 and 5.
        let block3 = arena.alloc_n(3, [1, 1]);
        let block5 = arena.alloc_n(5, [2, 2]);

        arena.free_n(block3, 3);
        arena.free_n(block5, 5);

        // alloc_n(4, …) should NOT reuse size-3 or size-5 blocks — must append.
        let block4 = arena.alloc_n(4, [3, 3]);
        assert_eq!(block4.to_usize(), 8); // appended after existing slots
        assert_eq!(arena.capacity(), 12); // 3 + 5 + 4, no reuse

        // alloc_n(3, …) reuses the freed size-3 block.
        let reused3 = arena.alloc_n(3, [4, 4]);
        assert_eq!(reused3, block3);

        // alloc_n(5, …) reuses the freed size-5 block.
        let reused5 = arena.alloc_n(5, [5, 5]);
        assert_eq!(reused5, block5);
    }

    #[test]
    fn free_n_multiple_blocks_same_size() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let a = arena.alloc_n(4, [1, 1]); // 0..4
        let b = arena.alloc_n(4, [2, 2]); // 4..8
        let c = arena.alloc_n(4, [3, 3]); // 8..12

        // Free all three — they form a LIFO chain on the size-4 free list.
        arena.free_n(a, 4);
        arena.free_n(b, 4);
        arena.free_n(c, 4);

        // alloc_n(4) reuses in LIFO order: c first, then b, then a.
        let first = arena.alloc_n(4, [10, 10]);
        assert_eq!(first, c); // most recently freed
        let second = arena.alloc_n(4, [20, 20]);
        assert_eq!(second, b);
        let third = arena.alloc_n(4, [30, 30]);
        assert_eq!(third, a);
    }

    #[test]
    fn free_n_partial_range() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let _a = arena.alloc([1, 0]);  // 0
        let start = arena.alloc_n(3, [2, 0]); // 1, 2, 3
        let _b = arena.alloc([4, 0]);  // 4
        assert_eq!(arena.len(), 5);

        arena.free_n(start, 3); // free indices 1, 2, 3 as a block
        assert_eq!(arena.len(), 2);

        // Surrounding allocations still valid
        assert_eq!(*arena.get(u32::from_usize(0)), [1, 0]);
        assert_eq!(*arena.get(u32::from_usize(4)), [4, 0]);

        // alloc_n(3, …) reuses the freed block.
        let reused = arena.alloc_n(3, [55, 0]);
        assert_eq!(reused, start);
        for i in 0..3 {
            assert_eq!(*arena.get(u32::from_usize(1 + i)), [55, 0]);
        }
    }

    #[test]
    fn free_n_mixed_single_and_block() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        // Single-slot free list and block free lists are independent.
        let single = arena.alloc([1, 0]); // 0
        let block = arena.alloc_n(3, [2, 0]); // 1, 2, 3
        let _other = arena.alloc([5, 0]); // 4

        arena.free(single); // goes to single-slot free list
        arena.free_n(block, 3); // goes to block[3] free list

        // Single alloc reuses the freed single slot.
        let reused_single = arena.alloc([10, 0]);
        assert_eq!(reused_single, single);

        // Block alloc_n(3) reuses the freed block.
        let reused_block = arena.alloc_n(3, [20, 0]);
        assert_eq!(reused_block, block);
    }

    #[test]
    #[should_panic(expected = "cannot allocate 0 slots")]
    fn alloc_n_zero_panics() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        arena.alloc_n(0, [0, 0]);
    }

    #[test]
    #[should_panic(expected = "cannot allocate 0 slots")]
    fn alloc_slice_empty_panics() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        arena.alloc_slice(&[]);
    }

    #[test]
    #[should_panic(expected = "cannot free 0 slots")]
    fn free_n_zero_panics() {
        let mut arena: Arena<[u32; 2], u32> = Arena::new();
        let s = arena.alloc([1, 2]);
        arena.free_n(s, 0);
    }

    #[test]
    fn get_range_basic() {
        let mut arena: Arena<u32, u32> = Arena::new();
        let start = arena.alloc_n(4, 0);
        // Mutate through get_range_mut
        let slice = arena.get_range_mut(start, 4);
        slice[0] = 10;
        slice[1] = 20;
        slice[2] = 30;
        slice[3] = 40;
        // Read through get_range
        let slice = arena.get_range(start, 4);
        assert_eq!(slice, &[10, 20, 30, 40]);
    }

    #[test]
    fn get_range_after_singles() {
        let mut arena: Arena<u32, u32> = Arena::new();
        let _a = arena.alloc(100); // index 0
        let start = arena.alloc_n(3, 0); // indices 1, 2, 3
        let slice = arena.get_range_mut(start, 3);
        slice[0] = 1;
        slice[1] = 2;
        slice[2] = 3;
        // Verify single slot is unaffected
        assert_eq!(*arena.get(_a), 100);
        // Verify range
        assert_eq!(arena.get_range(start, 3), &[1, 2, 3]);
    }

    #[test]
    fn get_range_with_u16_index() {
        let mut arena: Arena<u16, u16> = Arena::new();
        let start = arena.alloc_n(5, 99);
        let slice = arena.get_range_mut(start, 5);
        for (i, slot) in slice.iter_mut().enumerate() {
            *slot = i as u16;
        }
        assert_eq!(arena.get_range(start, 5), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn get_range_on_reused_block() {
        let mut arena: Arena<u32, u32> = Arena::new();
        let block = arena.alloc_n(4, 0);
        arena.free_n(block, 4);
        let reused = arena.alloc_n(4, 0);
        assert_eq!(reused, block);
        // Mutate via get_range_mut on the reused block.
        let slice = arena.get_range_mut(reused, 4);
        slice[0] = 10;
        slice[1] = 20;
        slice[2] = 30;
        slice[3] = 40;
        assert_eq!(arena.get_range(reused, 4), &[10, 20, 30, 40]);
    }
}