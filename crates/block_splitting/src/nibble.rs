use std::fmt;
use std::hash::Hash;
use std::ops::{Add, BitAnd, BitOr, BitXor, Mul, Not, Shl, Shr, Sub};

/// 4-bit unsigned integer. Inner value invariant: always masked to low 4 bits (0..=15).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Nibble(pub u8);

impl Nibble {
    pub const BIT_WIDTH: u8 = 4;
    pub const CAP: usize = 1 << Self::BIT_WIDTH;
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1);
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
