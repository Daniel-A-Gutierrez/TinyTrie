```rust
//!numeric trait ladder + type-level const facts (`MIDPOINT` neutral anchor,
//!`ZERO`/`ONE`/`MIN`/`MAX`/`BIT_WIDTH`) + wrapping/rotate ops, macro-impl'd
//!(`impl_num`/`impl_signed`/`impl_unsigned`/`impl_block_index`/`impl_signed_index`)
//!for the integer primitives. foundation for all address math; upholds only the
//!numeric contract.
///L0013
///common numeric ops + const facts + `rotate_left`/`rotate_right`/`wrapping_*`.
///no `Neg` (that lives on `SignedNum`).
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
    const ZERO: Self;
    const ONE: Self;
    const MIN: Self;
    const MAX: Self;
    const BIT_WIDTH: u8;
    fn rotate_left(self, n: u32) -> Self;
    fn rotate_right(self, n: u32) -> Self;
    fn wrapping_add(self, rhs: Self) -> Self;
    fn wrapping_sub(self, rhs: Self) -> Self;
    fn wrapping_shl(self, n: u32) -> Self;
    fn wrapping_shr(self, n: u32) -> Self;
}
///L0059
///signed `Num` + `Neg` — adds negation + `isize` conversion (signed addresses
///convert through `isize`, never `usize`).
pub trait SignedNum: Num + Neg<Output = Self> {
    fn as_isize(self) -> isize;
    fn from_isize(n: isize) -> Self;
}
///L0066
///unsigned `Num` — adds `usize` conversion (direct Vec/slot indexing).
pub trait UnsignedNum: Num {
    fn as_usize(self) -> usize;
    fn from_usize(n: usize) -> Self;
}
///L0074
///unsigned in-block ptr with an associated `Half` (overprovisioning sibling).
///impl'd for u16 and u32 (64-bit).
pub trait BlockIndex: UnsignedNum {
    type Half: UnsignedNum;
    fn as_halfptr(self) -> Self::Half;
    fn from_halfptr(half: Self::Half) -> Self;
}
///L0083
///signed in-block ptr with an associated `Half` (overprovisioning sibling).
pub trait SignedBlockIndex: SignedNum {
    type Half: SignedNum;
    fn as_halfptr(self) -> Self::Half;
    fn from_halfptr(half: Self::Half) -> Self;
}
///L0091
macro_rules! impl_num;
///L0110
macro_rules! impl_signed;
///L0119
macro_rules! impl_unsigned;
///L0128
macro_rules! impl_block_index;
///L0145
macro_rules! impl_signed_index;
///L0163
impl_num!(
    (i8, 0),
    (i16, 0),
    (i32, 0),
    (i64, 0),
    (u8, (<u8>::MAX >> 1) + 1),
    (u16, (<u16>::MAX >> 1) + 1),
    (u32, (<u32>::MAX >> 1) + 1),
    (u64, (<u64>::MAX >> 1) + 1),
);
///L0174
impl_signed!(i8, i16, i32, i64);
///L0176
impl_unsigned!(u8, u16, u32, u64);
///L0178
impl_block_index!(u16, u8);
///L0180
impl_signed_index!(i16, i8);
///L0183
#[cfg(target_pointer_width = "64")]
impl_block_index!(u32, u16);
///L0186
#[cfg(target_pointer_width = "64")]
impl_signed_index!(i32, i16);
```
