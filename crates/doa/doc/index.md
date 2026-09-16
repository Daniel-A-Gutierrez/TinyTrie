```rust
//!numeric trait ladder + type-level const facts (`MIDPOINT` neutral anchor,
//!`ZERO`/`ONE`/`MIN`/`MAX`/`BIT_WIDTH`) + wrapping/rotate ops, macro-impl'd
//!(`impl_num`/`impl_unsigned`/`impl_addr`) for the integer primitives.
//!foundation for all address math; upholds only the numeric contract.
///L0011
///common numeric ops + const facts + `rotate_left`/`rotate_right`/`wrapping_*`.
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
    /// Neutral address — where addresses anchor so growth has room both ways.
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
///L0056
///unsigned `Num` — adds `usize` conversion (direct Vec/slot indexing).
pub trait UnsignedNum: Num {
    fn as_usize(self) -> usize;
    fn from_usize(n: usize) -> Self;
}
///L0064
///unsigned in-block address with an associated `Half` (overprovisioning
///sibling). impl'd for u16 and u32 (64-bit).
pub trait Addr: UnsignedNum {
    type Half: UnsignedNum;
    fn as_half(self) -> Self::Half;
    fn from_half(half: Self::Half) -> Self;
}
///L0072
macro_rules! impl_num;
///L0091
macro_rules! impl_unsigned;
///L0100
macro_rules! impl_addr;
///L0118
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
///L0129
impl_unsigned!(u8, u16, u32, u64);
///L0131
impl_addr!(u16, u8);
///L0134
#[cfg(target_pointer_width = "64")]
impl_addr!(u32, u16);
```
