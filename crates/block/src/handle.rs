//!pointer handles over block-relative addresses: `S<T>` names stack bytes
//!front-relative, `H<T>` names heap bytes back-relative — distinct types so
//!neither is accepted where the other is expected; both survive `grow`.
//!`H<T>` region math: bytes `[cap − .off, cap − .off + .len)` of the data
//!region. the type parameter is the region's element type and the handles
//!are UNFORGEABLE (crate-private fields, constructible only by Block): a
//!value of `H<T>` proves its region was T-aligned at allocation, so typed
//!reads need no runtime alignment check. offsets are `u16` — the crate is
//!fixed-width (a growable wide-pointer variant would be a sibling crate).
//!HANDLE RECORD LAW: handles are POD with EXPLICIT padding only — every byte
//!defined; pad fields (`Pad`) are zero-written by archive paths so pushed
//!records never carry undefined (dirty) bytes.

use std::hash::Hash;
use std::marker::PhantomData;

///explicit handle padding: N bytes whose only job is to be defined-and-zero —
///archive paths write `default()` (all-zero), records read zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Pad<const N: usize>([u8; N]);

impl<const N: usize> Default for Pad<N> {
    fn default() -> Self {
        Pad([0; N])
    }
}

///SAFETY: the only field is a byte array — every byte is a field.
unsafe impl<const N: usize> ZeroPad for Pad<N> {}
///SAFETY: declared #[repr(C)] above.
unsafe impl<const N: usize> ReprC for Pad<N> {}

///marker: every byte of any constructed value is DEFINED — no implicit
///padding (pads are named `Pad` fields), so a bitwise copy of the value
///cannot carry uninitialized bytes into a record.
///SAFETY: the impl author guarantees no byte of the type is implicit
///padding (or that construction defines all bytes before any copy).
pub unsafe trait ZeroPad {}

///marker: `#[repr(C)]` — fixed field order for the serialized record form.
///SAFETY: the impl author guarantees the type is #[repr(C)].
pub unsafe trait ReprC {}

macro_rules! impl_markers {
    ($($t:ty),* $(,)?) => {$(
        // SAFETY: primitives carry no padding bytes.
        unsafe impl ZeroPad for $t {}
        // SAFETY: primitive layouts are their repr(C) layouts.
        unsafe impl ReprC for $t {}
    )*};
}

impl_markers!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, char);

///stack pointer: (start byte offset into the data region, byte len), naming
///a region of `T` pushed on the stack front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct S<T> {
    pub(crate) off: u16,
    pub(crate) len: u16,
    _t:             PhantomData<T>,
}

impl<T> S<T> {
    pub(crate) fn new(off: u16, len: u16) -> Self {
        S { off, len, _t: PhantomData }
    }

    ///start byte offset into the data region.
    pub fn off(&self) -> u16 {
        self.off
    }

    ///region byte length.
    pub fn len(&self) -> u16 {
        self.len
    }
}

/// SAFETY: two u16s and a zero-size marker — every byte is a field.
unsafe impl<T> ZeroPad for S<T> {}
/// SAFETY: declared #[repr(C)] above.
unsafe impl<T> ReprC for S<T> {}

///heap pointer: (back-to-start offset, byte len); region =
///`[cap − .off, cap − .off + .len)` of the data region, holding `T`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct H<T> {
    pub(crate) off: u16,
    pub(crate) len: u16,
    _t:             PhantomData<T>,
}

impl<T> H<T> {
    pub(crate) fn new(off: u16, len: u16) -> Self {
        H { off, len, _t: PhantomData }
    }

    ///distance from the data region's back edge to the region's start.
    pub fn off(&self) -> u16 {
        self.off
    }

    ///region byte length.
    pub fn len(&self) -> u16 {
        self.len
    }
}

/// SAFETY: two u16s and a zero-size marker — every byte is a field.
unsafe impl<T> ZeroPad for H<T> {}
/// SAFETY: declared #[repr(C)] above.
unsafe impl<T> ReprC for H<T> {}
