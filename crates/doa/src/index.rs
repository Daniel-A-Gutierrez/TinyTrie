use std::fmt;
use std::hash::Hash;
use std::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Not, Rem, Shl, Shr, Sub};

/// A natively-signed integer usable as an arena address.
///
/// `Self` is the signed address type; `Unsigned` is its magnitude type
/// (`i16` -> `u16`, `i32` -> `u32`). The supertrait list is the set of things a
/// real signed integer can do — arithmetic, bitwise ops, shifts, ordering — so
/// address math on the per-read path compiles to single native instructions
/// with no `as usize` detours or trait-method dispatch.
///
/// `min`/`max` return `Self` (the native signed min/max), not `usize`: the
/// pointer is signed, and `i16::MIN` is a perfectly representable `Self` value,
/// so the bottom of the address space needs no `MAX + 1` magnitude hack.
pub trait BlockIndex:
    Copy
        + Clone
        + PartialEq
        + Eq
        + PartialOrd
        + Ord
        + Hash
        + fmt::Debug
        + 'static
        + Add<Output = Self>
        + Sub<Output = Self>
        + Mul<Output = Self>
        + Div<Output = Self>
        + Rem<Output = Self>
        + BitAnd<Output = Self>
        + BitOr<Output = Self>
        + BitXor<Output = Self>
        + Not<Output = Self>
        + Shl<u32, Output = Self>
        + Shr<u32, Output = Self>
{
    /// Additive identity (`0`).
    const ZERO: Self;
    /// Multiplicative / increment identity (`1`).
    const ONE: Self;

    /// Native signed minimum (`i16::MIN` for `i16`).
    fn min() -> Self;
    /// Native signed maximum (`i16::MAX` for `i16`).
    fn max() -> Self;
    /// Bit width (`i16` -> 16).
    fn width() -> u8;
    //max of half ptr (256 for u16, 16 for u8)
    fn sqrt_max() -> Self {Self::ONE << (Self::width() as u32/2) - 1}
    /// Build `+n` or `-n` from a magnitude. May truncate or overflow-panic in
    /// debug for out-of-range `n`.
    fn from_usize(n: usize) -> Self;

    fn as_usize(self) -> usize;
}

macro_rules! impl_block_index {
    ($($t:ty),* $(,)?) => {
        $(
            impl BlockIndex for $t {
                const ZERO: Self = 0;
                const ONE: Self = 1;
                #[inline]
                fn min() -> Self {
                    <$t>::MIN
                }
                #[inline]
                fn max() -> Self {
                    <$t>::MAX
                }
                #[inline]
                fn width() -> u8 {
                    (std::mem::size_of::<$t>() * 8) as u8
                }
                #[inline]
                fn from_usize(n: usize) -> Self {
                    n as $t
                }
                #[inline]
                fn as_usize(self) -> usize {
                    self as usize
                }
            }
        )*
    };
}

impl_block_index!(u8, u16, u32, u64, u128, usize);

/// A natively-unsigned integer usable as an address magnitude / slot index.
///
/// Sealed: only the primitive unsigned types paired with a [`BlockIndex`]
/// qualify. Bounded to the same numeric/bitwise ops as [`BlockIndex`] plus
/// `as_usize`/`from_usize` for the (unavoidable) `Vec` boundary.
mod private {
    pub trait Sealed {}
}
pub trait UnsignedIndex:
    private::Sealed
        + Copy
        + Clone
        + PartialEq
        + Eq
        + PartialOrd
        + Ord
        + Hash
        + fmt::Debug
        + Default
        + 'static
        + Add<Output = Self>
        + Sub<Output = Self>
        + Mul<Output = Self>
        + Div<Output = Self>
        + Rem<Output = Self>
        + BitAnd<Output = Self>
        + BitOr<Output = Self>
        + BitXor<Output = Self>
        + Not<Output = Self>
        + Shl<u32, Output = Self>
        + Shr<u32, Output = Self>
{
    /// Lossless conversion to `usize` (for `Vec` indexing).
    fn as_usize(self) -> usize;
    /// Conversion from `usize`. Truncates (wraps) like `n as Self`.
    fn from_usize(n: usize) -> Self;
}

macro_rules! impl_unsigned_index {
    ($($t:ty),* $(,)?) => {
        $(
            impl private::Sealed for $t {}
            impl UnsignedIndex for $t {
                #[inline]
                fn as_usize(self) -> usize { self as usize }
                #[inline]
                fn from_usize(n: usize) -> Self { n as Self }
            }
        )*
    };
}

impl_unsigned_index!(u8, u16, u32, u64, u128, usize);