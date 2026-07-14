use std::fmt;
use std::hash::Hash;
use std::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Not, Rem, Shl, Shr, Sub};

/// A natively-signed integer usable as an arena address.
/// The valid address space is NOT Min..=Max, its -Max..Max
pub trait SignedBlockIndex:
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
    + std::ops::Neg
{
    const ZERO: Self;
    const ONE: Self;
    fn MIN() -> Self;
    fn MAX() -> Self;
    fn bit_width() -> u8;
    fn from_usize(n: usize) -> Self;
    fn as_usize(self) -> usize;
    fn as_isize(self) -> isize;
}

macro_rules! impl_block_index {
    ($($t:ty),* $(,)?) => {
        $(
            impl SignedBlockIndex for $t {
                const ZERO: Self = 0;
                const ONE: Self = 1;
                #[inline]
                fn MIN() -> Self {
                    -<$t>::MAX
                }
                #[inline]
                fn MAX() -> Self {
                    <$t>::MAX
                }
                #[inline]
                fn bit_width() -> u8 {
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
                #[inline]
                fn as_isize(self) -> isize {
                    self as isize
                }
            }
        )*
    };
}

impl_block_index!(i8, i16, i32);

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
