//! A fixed-capacity array with a stored length, backed by `[MaybeUninit<T>; N]`.
//!
//! Slots `[0..len)` are always initialized. `TinyArray` owns Drop for those slots.
//! Copied from `crates/btrees/src/tiny_array.rs` as an interim local copy for the
//! FlatNode work; the plan is to extract it into a shared dependency later.

use std::mem::MaybeUninit;

/// A fixed-capacity array with a stored length.
///
/// Slots `[0..len)` are initialized. `N` is the capacity (max 255 since `len` is `u8`).
/// `TinyArray` is `Copy` (requires `T: Copy`): it holds no heap allocation — just an
/// inline `[MaybeUninit<T>; N]` plus a `u8` length — so copying it is a plain bit copy
/// with no `Drop` side effects. Because `T: Copy`, there is nothing to drop, so (unlike
/// the upstream `crates/btrees` copy) this one has no `Drop` impl.
///
/// `Clone`/`Copy` are written by hand (not derived): `MaybeUninit<T>: Clone` requires
/// `T: Copy`, so deriving `Clone` would force `T: Copy` onto unrelated method impls.
/// The manual impls gate only on `T: Copy`.
pub struct TinyArray<T, const N: usize>
where
    [(); N]:
{
    len: u8,
    slots: [MaybeUninit<T>; N],
}

impl<T: Copy, const N: usize> Clone for TinyArray<T, N>
where
    [(); N]:
{
    #[inline]
    fn clone(&self) -> Self {
        // `Copy` makes clone a plain bit copy.
        *self
    }
}

impl<T: Copy, const N: usize> Copy for TinyArray<T, N> where [(); N]: {}

impl<T: std::fmt::Debug, const N: usize> std::fmt::Debug for TinyArray<T, N>
where
    [(); N]:
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

#[allow(dead_code)]
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
    /// Panics if the array is full or `pos > len`. Increments `len` by 1.
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
    /// Returns the removed element. Decrements `len` by 1.
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

    /// Append `val` to the end of the array. Panics if the array is full.
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

    /// Set `len` to `new_len` without dropping any elements. The caller is
    /// responsible for the truncated elements (e.g. moved elsewhere during a split).
    #[inline]
    pub fn truncate(&mut self, new_len: u8) {
        debug_assert!(new_len as usize <= N, "TinyArray::truncate: new_len exceeds capacity");
        self.len = new_len;
    }

    /// Reorder initialized elements so slot `i` holds what was at slot `perm[i]`.
    /// `perm` must be a permutation of `0..len` (only its first `len` entries read).
    /// Cycle-following in-place `ptr::swap` — no `T: Clone`, no double-drop.
    pub fn permute_in_place(&mut self, perm: &[usize]) {
        let n = self.len as usize;
        if n <= 1 {
            return;
        }
        let mut p: [usize; N] = [0; N];
        for i in 0..n {
            p[i] = perm[i];
            debug_assert!(perm[i] < n, "TinyArray::permute_in_place: perm out of range");
        }
        for i in 0..n {
            let mut j = i;
            while p[j] != i {
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

    /// Read element at `pos` without removing it or shifting. The slot is left
    /// uninitialized; caller must avoid double-drop.
    ///
    /// # Safety
    ///
    /// `pos < self.len()`.
    #[inline]
    pub unsafe fn read_slot(&mut self, pos: usize) -> T {
        debug_assert!(pos < self.len as usize, "TinyArray::read_slot: index out of bounds");
        unsafe { self.slots[pos].assume_init_read() }
    }

    /// Move elements `[from..len)` into `dst` starting at dst index 0. `self` is
    /// truncated to `from` (moved elements are not dropped). Core split op.
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

    /// Move `self[from..len)` to **dst's front** (prepend), shifting dst's existing
    /// elements right. `self` is truncated to `from`.
    pub fn drain_into_front(&mut self, from: usize, dst: &mut Self) {
        let count = self.len as usize - from;
        debug_assert!(from < self.len as usize, "TinyArray::drain_into_front: empty drain");
        debug_assert!(
            dst.len as usize + count <= N,
            "TinyArray::drain_into_front: dst overflow"
        );
        unsafe {
            if dst.len as usize != 0 {
                std::ptr::copy(
                    dst.slots[0].as_ptr(),
                    dst.slots[count].as_mut_ptr(),
                    dst.len as usize,
                );
            }
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
    /// `[count..len)` left to the front.
    pub fn drain_front_into(&mut self, count: usize, dst: &mut Self) {
        debug_assert!(count <= self.len as usize, "TinyArray::drain_front_into: count out of bounds");
        debug_assert!(
            dst.len as usize + count <= N,
            "TinyArray::drain_front_into: dst overflow"
        );
        let l = self.len as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.slots[0].as_ptr(),
                dst.slots[dst.len as usize].as_mut_ptr(),
                count,
            );
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

impl<T, const N: usize> Default for TinyArray<T, N>
where
    [(); N]:
{
    fn default() -> Self {
        Self::new()
    }
}