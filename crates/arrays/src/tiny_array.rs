//! A fixed-capacity array with a stored length, backed by `[MaybeUninit<T>; N]`.
//!
//! Self-contained local copy (no external deps) for the leaf-node benchmark.
//! Slots `[0..len)` are always initialized. `Copy` when `T: Copy` (no heap).

use std::mem::MaybeUninit;

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
        *self
    }
}

impl<T: Copy, const N: usize> Copy for TinyArray<T, N> where [(); N]: {}

impl<T, const N: usize> TinyArray<T, N>
where
    [(); N]:
{
    pub fn new() -> Self {
        Self {
            len: 0,
            slots: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        self.len as usize == N
    }

    #[inline]
    pub fn get(&self, i: usize) -> &T {
        debug_assert!(i < self.len as usize, "TinyArray::get: oob");
        unsafe { &*self.slots[i].as_ptr() }
    }

    #[inline]
    pub fn get_mut(&mut self, i: usize) -> &mut T {
        debug_assert!(i < self.len as usize, "TinyArray::get_mut: oob");
        unsafe { &mut *self.slots[i].as_mut_ptr() }
    }

    /// Access the initialized region as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.slots[0].as_ptr(), self.len as usize) }
    }

    /// Insert `val` at `pos`, shifting `[pos..len)` right by one. Panics if full
    /// or `pos > len`. Increments `len`.
    pub fn insert_at(&mut self, pos: usize, val: T) {
        debug_assert!(!self.is_full(), "TinyArray::insert_at: full");
        debug_assert!(pos <= self.len as usize, "TinyArray::insert_at: pos oob");
        let l = self.len as usize;
        if pos < l {
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

    pub fn push(&mut self, val: T) {
        self.insert_at(self.len as usize, val);
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