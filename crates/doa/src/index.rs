use std::fmt;
use std::hash::Hash;
use std::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Neg, Not, Rem, Shl, Shr, Sub};

/// All the numeric operator/copy/ord bounds an index type needs, plus the
/// type-level facts (`MIN`/`MAX`/`bit_width`) and the address anchor `MIDPOINT`.
/// Everything common to every index — signed, unsigned, full, or small — lives
/// here, so a `Half` or a small-ptr block bounded only `Num` still gets the
/// anchor and the facts. No `Neg`/sign-conversion; those live on
/// `SignedNum`/`UnsignedNum`.
pub trait Num:
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
    /// Neutral address — where pointers anchor so growth has room both ways.
    /// Signed: `0`. Unsigned: range midpoint `(MAX >> 1) + 1` = `1 << (bit_width - 1)`.
    const MIDPOINT: Self;
    fn rotate_left(self, n: u32) -> Self;
    fn rotate_right(self, n: u32) -> Self;
    fn MIN() -> Self;
    fn MAX() -> Self;
    fn bit_width() -> u8;
}

/// Signed `Num` — adds negation and `isize` conversion. Signed addresses
/// convert through `isize`, never `usize`.
pub trait SignedNum: Num + Neg<Output = Self> {
    fn as_isize(self) -> isize;
    fn from_isize(n: isize) -> Self;
}

/// Unsigned `Num` — adds `usize` conversion (direct Vec/slot indexing).
pub trait UnsignedNum: Num {
    fn as_usize(self) -> usize;
    fn from_usize(n: usize) -> Self;
}

/// Unsigned block-internal pointer with a native half-width sibling (OVERP).
/// Essence: has a `Half`. The anchor (`MIDPOINT`) and facts come from `Num`;
/// `usize` conversion from `UnsignedNum`.
pub trait BlockIndex: UnsignedNum {
    type Half: UnsignedNum;
    fn as_halfptr(self) -> Self::Half;
    fn from_halfptr(half: Self::Half) -> Self;
}

/// Signed block-internal pointer with a native half-width sibling (OVERP).
pub trait SignedBlockIndex: SignedNum {
    type Half: SignedNum;
    fn as_halfptr(self) -> Self::Half;
    fn from_halfptr(half: Self::Half) -> Self;
}

macro_rules! impl_num {
    ($(($t:ty, $midpoint:expr)),* $(,)?) => {
        $( impl Num for $t {
            const MIDPOINT: Self = $midpoint;
            #[inline] fn rotate_left(self, n: u32) -> Self { <$t>::rotate_left(self, n) }
            #[inline] fn rotate_right(self, n: u32) -> Self { <$t>::rotate_right(self, n) }
            #[inline] fn MIN() -> Self { <$t>::MIN }
            #[inline] fn MAX() -> Self { <$t>::MAX }
            #[inline] fn bit_width() -> u8 { (std::mem::size_of::<$t>() * 8) as u8 }
        } )*
    };
}

macro_rules! impl_signed {
    ($($t:ty),* $(,)?) => {
        $( impl SignedNum for $t {
            #[inline] fn as_isize(self) -> isize { self as isize }
            #[inline] fn from_isize(n: isize) -> Self { n as $t }
        } )*
    };
}

macro_rules! impl_unsigned {
    ($($t:ty),* $(,)?) => {
        $( impl UnsignedNum for $t {
            #[inline] fn as_usize(self) -> usize { self as usize }
            #[inline] fn from_usize(n: usize) -> Self { n as $t }
        } )*
    };
}

macro_rules! impl_block_index {
    ($t:ty,$half:ty) => {
        impl BlockIndex for $t {
            type Half = $half;
            #[inline] fn as_halfptr(self) -> Self::Half { self as Self::Half }
            #[inline] fn from_halfptr(half: Self::Half) -> Self { half as Self }
        }
    };
}

macro_rules! impl_signed_index {
    ($t:ty,$half:ty) => {
        impl SignedBlockIndex for $t {
            type Half = $half;
            #[inline] fn as_halfptr(self) -> Self::Half { self as Self::Half }
            #[inline] fn from_halfptr(half: Self::Half) -> Self { half as Self }
        }
    };
}

impl_num!(
    (i8, 0), (i16, 0), (i32, 0), (i64, 0),
    (u8, (<u8>::MAX >> 1) + 1),
    (u16, (<u16>::MAX >> 1) + 1),
    (u32, (<u32>::MAX >> 1) + 1),
    (u64, (<u64>::MAX >> 1) + 1),
);
impl_signed!(i8, i16, i32, i64);
impl_unsigned!(u8, u16, u32, u64);

impl_block_index!(u16, u8);
impl_signed_index!(i16, i8);
#[cfg(target_pointer_width = "64")]
impl_block_index!(u32, u16);
#[cfg(target_pointer_width = "64")]
impl_signed_index!(i32, i16);

