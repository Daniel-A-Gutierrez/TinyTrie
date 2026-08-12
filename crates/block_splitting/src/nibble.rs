use std::fmt;
use std::hash::Hash;
use std::ops::{Add, BitAnd, BitOr, BitXor, Not, Shl, Shr, Sub, Mul};

/// 4-bit unsigned integer. Inner value invariant: always masked to low 4 bits (0..=15).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Nibble(pub u8);

impl Nibble {
    pub const BIT_WIDTH: u8 = 4;
    pub const CAP: usize = 1 << Self::BIT_WIDTH;
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1);
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(15);
    pub const MIDPOINT: Self = Self(1 << (Self::BIT_WIDTH - 1));

    pub const fn from_u8(v: u8) -> Self {
        Self(v & 0x0F)
    }

    pub const fn from_usize(v: usize) -> Self {
        Self((v as u8) & 0x0F)
    }

    pub const fn as_u8(self) -> u8 {
        self.0
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub const fn wrapping_add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0) & 0x0F)
    }

    pub const fn wrapping_sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0) & 0x0F)
    }

    pub const fn wrapping_mul(self, rhs: Self) -> Self {
        Self(self.0.wrapping_mul(rhs.0) & 0x0F)
    }

    /// Rotate left within the 4-bit window. `0b1001.rotate_left(1) == 0b0011`.
    pub const fn rotate_left(self, n: u32) -> Self {
        let n = (n as usize) % 4;
        let v = self.0;
        let r = ((v << n) | (v >> (4 - n))) & 0x0F;
        Self(r)
    }

    pub const fn rotate_right(self, n: u32) -> Self {
        let n = (n as usize) % 4;
        let v = self.0;
        let r = ((v >> n) | (v << (4 - n))) & 0x0F;
        Self(r)
    }

    /// Wrapping left shift: bits past bit 3 discarded, low bits 0.
    pub const fn wrapping_shl(self, n: u32) -> Self {
        Self((self.0 << n) & 0x0F)
    }

    /// Logical right shift: high bits 0.
    pub const fn wrapping_shr(self, n: u32) -> Self {
        Self((self.0 >> n) & 0x0F)
    }
}

impl From<u8> for Nibble {
    fn from(v: u8) -> Self {
        Self::from_u8(v)
    }
}

impl fmt::Debug for Nibble {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Nibble({})", self.0)
    }
}

impl fmt::Display for Nibble {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Add for Nibble {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        self.wrapping_add(rhs)
    }
}

impl Sub for Nibble {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        self.wrapping_sub(rhs)
    }
}

impl Mul for Nibble {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        self.wrapping_mul(rhs)
    }
}

impl BitAnd for Nibble {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitOr for Nibble {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitXor for Nibble {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self {
        Self(self.0 ^ rhs.0)
    }
}

impl Not for Nibble {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0 & 0x0F)
    }
}

impl Shl<u32> for Nibble {
    type Output = Self;
    fn shl(self, n: u32) -> Self {
        self.wrapping_shl(n)
    }
}

impl Shr<u32> for Nibble {
    type Output = Self;
    fn shr(self, n: u32) -> Self {
        self.wrapping_shr(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_masks() {
        assert_eq!(Nibble::from_u8(0xFF).as_u8(), 15);
        assert_eq!(Nibble::from_usize(17).as_u8(), 1);
        assert_eq!(Nibble::from(20u8).as_u8(), 4);
    }

    #[test]
    fn wrapping_add_overflow() {
        assert_eq!((Nibble::MAX + Nibble::ONE).as_u8(), 0);
        assert_eq!((Nibble(15) + Nibble(2)).as_u8(), 1);
    }

    #[test]
    fn wrapping_sub_underflow() {
        assert_eq!((Nibble::ZERO - Nibble::ONE).as_u8(), 15);
        assert_eq!((Nibble(2) - Nibble(5)).as_u8(), 13);
    }

    #[test]
    fn wrapping_mul() {
        assert_eq!((Nibble(3) * Nibble(5)).as_u8(), 15);
        assert_eq!((Nibble(4) * Nibble(4)).as_u8(), 0);
    }

    #[test]
    fn rotate_left_carry() {
        assert_eq!(Nibble(0b1001).rotate_left(1).as_u8(), 0b0011);
    }

    #[test]
    fn rotate_round_trip() {
        let v = Nibble(0b1011);
        assert_eq!(v.rotate_left(2).rotate_right(2), v);
        assert_eq!(v.rotate_left(5).rotate_right(5), v);
    }

    #[test]
    fn wrapping_shl_truncates() {
        assert_eq!((Nibble(15) << 1).as_u8(), 14);
        assert_eq!(Nibble(0b0111).wrapping_shl(1).as_u8(), 0b1110);
    }

    #[test]
    fn logical_shr() {
        assert_eq!((Nibble(0b1000) >> 1).as_u8(), 0b0100);
        assert_eq!(Nibble(0b1100).wrapping_shr(2).as_u8(), 0b0011);
    }

    #[test]
    fn bitwise() {
        assert_eq!((Nibble(0b1100) & Nibble(0b1010)).as_u8(), 0b1000);
        assert_eq!((Nibble(0b1100) | Nibble(0b1010)).as_u8(), 0b1110);
        assert_eq!((Nibble(0b1100) ^ Nibble(0b1010)).as_u8(), 0b0110);
        assert_eq!((!Nibble(0b0000)).as_u8(), 0b1111);
        assert_eq!((!Nibble(0b1010)).as_u8(), 0b0101);
    }

    #[test]
    fn midpoint() {
        assert_eq!(Nibble::MIDPOINT.as_u8(), 8);
    }
}