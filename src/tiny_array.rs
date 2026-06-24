//! A fixed-capacity array with a stored length, backed by `[MaybeUninit<T>; N]`.
//!
//! Slots `[0..len)` are always initialized. `TinyArray` owns Drop for those slots.
//! Designed as a building block for B+ tree node keys/values.

use std::mem::MaybeUninit;
use std::ptr::drop_in_place;

/// A fixed-capacity array with a stored length.
///
/// Slots `[0..len)` are initialized. `N` is the capacity (max 255 since `len` is `u8`).
/// Owns the Drop for initialized slots — when a `TinyArray` is dropped, all
/// initialized elements are dropped.
pub struct TinyArray<T, const N: usize>
where
    [(); N]:
{
    len: u8,
    slots: [MaybeUninit<T>; N],
}

impl<T, const N: usize> TinyArray<T, N>
where
    [(); N]:
{
    /// Create an empty `TinyArray` with `len == 0`.
    pub fn new() -> Self {
        Self {
            len: 0,
            slots: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    /// Number of initialized elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Is the array at capacity?
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len as usize == N
    }

    /// Access the initialized region as a slice.
    ///
    /// SAFETY: `slots[..len]` are all initialized.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.slots[0].as_ptr(), self.len as usize) }
    }

    /// Get a reference to element at `i`. Panics if out of bounds.
    #[inline]
    pub fn get(&self, i: usize) -> &T {
        debug_assert!(i < self.len as usize, "TinyArray::get: index out of bounds");
        unsafe { &*self.slots[i].as_ptr() }
    }

    /// Get a reference to element at `i` without bounds check.
    ///
    /// # Safety
    ///
    /// `i < self.len()`.
    #[inline]
    pub unsafe fn get_unchecked(&self, i: usize) -> &T {
        unsafe { &*self.slots[i].as_ptr() }
    }

    /// Get a mutable reference to element at `i`. Panics if out of bounds.
    #[inline]
    pub fn get_mut(&mut self, i: usize) -> &mut T {
        debug_assert!(i < self.len as usize, "TinyArray::get_mut: index out of bounds");
        unsafe { &mut *self.slots[i].as_mut_ptr() }
    }

    /// Insert `val` at position `pos`, shifting elements `[pos..len)` right by one.
    ///
    /// Panics if the array is full or `pos > len`.
    /// Increments `len` by 1.
    pub fn insert_at(&mut self, pos: usize, val: T) {
        debug_assert!(!self.is_full(), "TinyArray::insert_at: array is full");
        debug_assert!(pos <= self.len as usize, "TinyArray::insert_at: pos out of bounds");
        let l = self.len as usize;
        if pos < l {
            // Shift slots [pos..l] → [pos+1..l+1]
            unsafe {
                std::ptr::copy(
                    self.slots[pos].as_ptr(),
                    self.slots[pos + 1].as_mut_ptr(),
                    l - pos,
                );
            }
        }
        unsafe {
            self.slots[pos].as_mut_ptr().write(val);
        }
        self.len += 1;
    }

    /// Remove element at `pos`, shifting elements `[pos+1..len)` left by one.
    ///
    /// Returns the removed element.
    /// Decrements `len` by 1.
    pub fn remove_at(&mut self, pos: usize) -> T {
        debug_assert!(pos < self.len as usize, "TinyArray::remove_at: index out of bounds");
        let l = self.len as usize;
        let val = unsafe { self.slots[pos].assume_init_read() };
        if pos + 1 < l {
            unsafe {
                std::ptr::copy(
                    self.slots[pos + 1].as_ptr(),
                    self.slots[pos].as_mut_ptr(),
                    l - pos - 1,
                );
            }
        }
        self.len -= 1;
        val
    }

    /// Append `val` to the end of the array.
    ///
    /// Panics if the array is full.
    pub fn push(&mut self, val: T) {
        self.insert_at(self.len as usize, val);
    }

    /// Remove and return the last element, or `None` if empty.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.slots[self.len as usize].assume_init_read() })
    }

    /// Set `len` to `new_len` without dropping any elements.
    ///
    /// The caller is responsible for ensuring that truncated elements (if any)
    /// have been moved elsewhere (e.g., into a new node during a split).
    /// This does NOT drop the elements in `[new_len..old_len)`.
    #[inline]
    pub fn truncate(&mut self, new_len: u8) {
        debug_assert!(new_len as usize <= N, "TinyArray::truncate: new_len exceeds capacity");
        self.len = new_len;
    }

    /// Reorder the initialized elements so that slot `i` holds what was
    /// originally at slot `perm[i]` — i.e. `self[i] = old_self[perm[i]]`.
    ///
    /// `perm` must be a permutation of `0..len` (only its first `len` entries
    /// are read). Used by the fixed-form B+ tree to sort an unsorted leaf/inode
    /// in place before a split or rebalance: the caller computes a sorted index
    /// permutation by key and applies it to BOTH the keys array and the parallel
    /// values/ptrs array so they stay aligned.
    ///
    /// Implemented with cycle-following in-place slot swaps (`ptr::swap`), so it
    /// moves (never copies) elements — no `T: Clone` bound, no double-drop. `T`
    /// need not be `Copy`.
    pub fn permute_in_place(&mut self, perm: &[usize]) {
        let n = self.len as usize;
        if n <= 1 {
            return;
        }
        // Mutable working copy of the permutation; positions are marked settled
        // by writing their own index back into them.
        let mut p: [usize; N] = [0; N];
        for i in 0..n {
            p[i] = perm[i];
            debug_assert!(perm[i] < n, "TinyArray::permute_in_place: perm out of range");
        }
        for i in 0..n {
            let mut j = i;
            while p[j] != i {
                // SAFETY: slots j and p[j] are both < n, hence initialized.
                unsafe {
                    std::ptr::swap(
                        self.slots[j].as_mut_ptr(),
                        self.slots[p[j]].as_mut_ptr(),
                    );
                }
                let next = p[j];
                p[j] = j; // mark position j settled
                j = next;
            }
            p[j] = j;
        }
    }

    /// Read element at `pos` without removing it or shifting.
    ///
    /// The slot is left in an uninitialized state. The caller must ensure
    /// the element is not double-dropped (e.g., by truncating `len` past it
    /// or overwriting it).
    ///
    /// # Safety
    ///
    /// `pos < self.len()`. The caller must not use the slot after this call
    /// unless it is overwritten or `len` is adjusted past it.
    #[inline]
    pub unsafe fn read_slot(&mut self, pos: usize) -> T {
        debug_assert!(pos < self.len as usize, "TinyArray::read_slot: index out of bounds");
        unsafe { self.slots[pos].assume_init_read() }
    }

    /// Move elements `[from..len)` into `dst`, starting at `dst` index 0.
    ///
    /// After this call:
    /// - `dst` has `self.len - from` elements in slots `[0..self.len - from)`.
    /// - `self` is truncated to `from` (without dropping the moved elements).
    ///
    /// This is the core operation for B+ tree node splits: split the array at
    /// `from`, move the upper half into a new node's array, and truncate the
    /// original to the lower half.
    ///
    /// # Panics
    ///
    /// Panics if `from > self.len` or if `dst` doesn't have capacity for the
    /// moved elements (i.e., `self.len - from > N`).
    /// Move elements `[from..len)` into `dst`, starting at `dst` index 0.
    ///
    /// After this call:
    /// - `dst` has `self.len - from` elements in slots `[0..self.len - from)`.
    /// - `self` is truncated to `from` (without dropping the moved elements).
    ///
    /// Caller must ensure `from < self.len` so that at least one element is
    /// moved and the source index is in bounds.
    pub fn drain_into(&mut self, from: usize, dst: &mut Self) {
        let count = self.len as usize - from;
        debug_assert!(from < self.len as usize, "TinyArray::drain_into: empty drain");
        debug_assert!(dst.len as usize + count <= N, "TinyArray::drain_into: dst overflow");
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.slots[from].as_ptr(),
                dst.slots[dst.len as usize].as_mut_ptr(),
                count,
            );
        }
        dst.len += count as u8;
        self.len = from as u8;
    }

    /// Move `self[from..len)` to **dst's front** (prepend), shifting dst's
    /// existing elements right. After this call:
    /// - `dst` has `dst.len + (self.len - from)` elements, with the moved range
    ///   at indices `[0 .. self.len - from)` and the old dst elements after it.
    /// - `self` is truncated to `from` (without dropping the moved elements).
    ///
    /// Symmetric to [`drain_into`](Self::drain_into), which appends to dst's
    /// *end*. Used by B+ tree leaf rebalance to move a full leaf's tail to its
    /// right sibling's front (all moved keys are smaller than the sibling's).
    ///
    /// Caller must ensure `from < self.len` and dst has capacity for the moved
    /// elements.
    pub fn drain_into_front(&mut self, from: usize, dst: &mut Self) {
        let count = self.len as usize - from;
        debug_assert!(from < self.len as usize, "TinyArray::drain_into_front: empty drain");
        debug_assert!(
            dst.len as usize + count <= N,
            "TinyArray::drain_into_front: dst overflow"
        );
        unsafe {
            // Shift dst's initialized elements [0..dst.len) right by `count`.
            if dst.len as usize != 0 {
                std::ptr::copy(
                    dst.slots[0].as_ptr(),
                    dst.slots[count].as_mut_ptr(),
                    dst.len as usize,
                );
            }
            // Copy self's tail into the freed front of dst.
            std::ptr::copy_nonoverlapping(
                self.slots[from].as_ptr(),
                dst.slots[0].as_mut_ptr(),
                count,
            );
        }
        dst.len += count as u8;
        self.len = from as u8;
    }

    /// Move `self[0..count)` to **dst's end** (append), then shift self's
    /// remaining elements `[count..len)` left to the front. After this call:
    /// - `dst` has `dst.len + count` elements, with the moved range appended.
    /// - `self` has `self.len - count` elements (its old `[count..len)` range
    ///   now at the front), and is not dropped.
    ///
    /// Symmetric to [`drain_into`](Self::drain_into), which drains the *tail*.
    /// Used by B+ tree leaf rebalance to move a full leaf's front to its left
    /// sibling's end (all moved keys are larger than the sibling's).
    ///
    /// Caller must ensure `count <= self.len` and dst has capacity.
    pub fn drain_front_into(&mut self, count: usize, dst: &mut Self) {
        debug_assert!(count <= self.len as usize, "TinyArray::drain_front_into: count out of bounds");
        debug_assert!(
            dst.len as usize + count <= N,
            "TinyArray::drain_front_into: dst overflow"
        );
        let l = self.len as usize;
        unsafe {
            // Append self's front to dst's end.
            std::ptr::copy_nonoverlapping(
                self.slots[0].as_ptr(),
                dst.slots[dst.len as usize].as_mut_ptr(),
                count,
            );
            // Shift self's [count..len) left to [0..len-count).
            if count < l {
                std::ptr::copy(
                    self.slots[count].as_ptr(),
                    self.slots[0].as_mut_ptr(),
                    l - count,
                );
            }
        }
        dst.len += count as u8;
        self.len -= count as u8;
    }
}

impl<T, const N: usize> Drop for TinyArray<T, N>
where
    [(); N]:
{
    fn drop(&mut self) {
        for i in 0..self.len as usize {
            unsafe {
                drop_in_place(self.slots[i].as_mut_ptr());
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/tiny_array.rs"]
mod tests;