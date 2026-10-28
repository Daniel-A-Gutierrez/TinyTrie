//!pointer handles over block-relative addresses: `S` names stack bytes
//!front-relative, `H` names heap bytes back-relative — distinct types so
//!neither is accepted where the other is expected; both survive `grow`.
//!`H` region math: bytes `[cap − .0, cap − .0 + .1)` of the data region.
//!HANDLE LAYOUT LAW: handles are POD with EXPLICIT padding only — every byte
//!defined; pad fields (`Pad`, `P::TailPad2/3`) are zero-written by archive
//!paths so pushed records never carry undefined (dirty) bytes.

use std::fmt;
use std::hash::Hash;

///explicit handle padding: N bytes whose only job is to be defined-and-zero —
///archive paths write `default()` (all-zero), records read zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad<const N: usize>([u8; N]);

impl<const N: usize> Default for Pad<N> {
    fn default() -> Self {
        Pad([0; N])
    }
}

///integer width for block-relative pointers; all math runs in `usize`,
///values converted at API boundaries (`from_usize` callers guarantee n ≤ MAX).
pub trait Ptr: Copy + PartialEq + Eq + PartialOrd + Ord + Hash + fmt::Debug + 'static {
    const MAX: Self;
    ///pointer width in bytes (1/2/4/8) — monomorphized const arithmetic.
    const BYTES: usize;
    ///explicit trailing pad a 2·P-byte tail field needs after an 8-aligned
    ///field (u8/u16/u32/u64 widths → 6/4/0/0). array lengths can't compute
    ///`P::BYTES` (generic const exprs), so pads ride per-width types.
    type TailPad2: Copy + Clone + Eq + Default + fmt::Debug;
    ///same for a 3·P-byte tail field (→ 5/2/4/0).
    type TailPad3: Copy + Clone + Eq + Default + fmt::Debug;
    fn as_usize(self) -> usize;
    fn from_usize(n: usize) -> Self;
}

macro_rules! impl_ptr {
    ($($t:ty, $bytes:expr, $p2:ty, $p3:ty;)*) => {
        $( impl Ptr for $t {
            const MAX: Self = <$t>::MAX;
            const BYTES: usize = $bytes;
            type TailPad2 = $p2;
            type TailPad3 = $p3;
            #[inline] fn as_usize(self) -> usize { self as usize }
            #[inline] fn from_usize(n: usize) -> Self { n as $t }
        } )*
    };
}

impl_ptr!(
    u8, 1, Pad<6>, Pad<5>;
    u16, 2, Pad<4>, Pad<2>;
    u32, 4, Pad<0>, Pad<4>;
    u64, 8, Pad<0>, Pad<0>;
    usize, 8, Pad<0>, Pad<0>;
);

///stack pointer: (start byte offset into the data region, byte len).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct S<P>(pub P, pub P);

///heap pointer: (back-to-start offset, byte len); region = `[cap − .0, cap − .0 + .1)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct H<P>(pub P, pub P);