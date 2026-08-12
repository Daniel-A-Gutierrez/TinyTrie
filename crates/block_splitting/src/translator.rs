use std::fmt;

use crate::nibble::Nibble;

/// Debug translator: concrete fields, no fn-ptr specialization. Mirrors doa's
/// `p2v`/`v2p` math over the 4-bit address space (phys and virt are both `Nibble`,
/// 0..16, all arithmetic wrapping).
///
/// `inner_offset` lives in PHYSICAL space (added before the shift).
/// `outer_offset` lives in VIRTUAL space (added after the rotation).
/// p2v rotates RIGHT so the vaddr follows the physical spread; v2p is the exact
/// inverse on canonical (block-handed-out) vaddrs.
#[derive(Clone, Copy, Debug)]
pub struct Translator {
    pub inner_offset: Nibble,
    pub outer_offset: Nibble,
    pub shift: u32,
    pub rotation: u32,
}

impl Translator {
    pub const fn new(inner_offset: Nibble, outer_offset: Nibble, shift: u32, rotation: u32) -> Self {
        Self { inner_offset, outer_offset, shift, rotation }
    }

    /// phys slot -> vaddr.
    pub fn p2v(&self, phys: Nibble) -> Nibble {
        let x = phys.wrapping_add(self.inner_offset);
        let x = x.wrapping_shl(self.shift);
        let x = x.rotate_right(self.rotation);
        x.wrapping_add(self.outer_offset)
    }

    /// vaddr -> phys slot. Exact inverse of `p2v` on canonical vaddrs.
    pub fn v2p(&self, virt: Nibble) -> Nibble {
        let x = virt.wrapping_sub(self.outer_offset);
        let x = x.rotate_left(self.rotation);
        let x = x.wrapping_shr(self.shift);
        x.wrapping_sub(self.inner_offset)
    }

    /// Update params for a spread where phys `i -> 2i + offset` and `shift` drops
    /// by 1. Sets `inner_offset = 2*inner - offset` (wrapping) so handed-out vaddrs
    /// stay stable; `outer`/`rotation` unchanged. Panics if `shift == 0`.
    pub fn spread(&mut self, offset: bool) {
        assert!(self.shift != 0, "spread: shift == 0");
        let inner = self.inner_offset.as_u8();
        self.inner_offset = Nibble::from_u8((2 * inner).wrapping_sub(offset as u8) & 0x0F);
        self.shift -= 1;
    }

    /// Bump `rotation` by 1 (the split partner to `spread`).
    pub fn rotate(&mut self) {
        self.rotation = self.rotation.wrapping_add(1) % Nibble::BIT_WIDTH as u32;
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new(Nibble::ZERO, Nibble::ZERO, 0, 0)
    }
}

impl fmt::Display for Translator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "inner={} outer={} shift={} rot={}",
            self.inner_offset, self.outer_offset, self.shift, self.rotation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(inner: u8, outer: u8, shift: u32, rot: u32) -> Translator {
        Translator::new(Nibble::from_u8(inner), Nibble::from_u8(outer), shift, rot)
    }

    #[test]
    fn identity() {
        let tr = t(0, 0, 0, 0);
        for p in 0..16 {
            assert_eq!(tr.p2v(Nibble::from_u8(p)).as_u8(), p);
            assert_eq!(tr.v2p(Nibble::from_u8(p)).as_u8(), p);
        }
    }

    // shift==0 => p2v is a bijection on all 16 (wrapping_add + rotate are bijections;
    // shl is the only non-injective step). v2p inverts it exactly.
    #[test]
    fn shift_zero_is_bijection() {
        for inner in 0..16u8 {
            for outer in 0..16u8 {
                for rot in 0..4u32 {
                    let tr = t(inner, outer, 0, rot);
                    for p in 0..16u8 {
                        let v = tr.p2v(Nibble::from_u8(p));
                        assert_eq!(tr.v2p(v).as_u8(), p, "inner={inner} outer={outer} rot={rot} phys={p}");
                    }
                }
            }
        }
    }

    // shift>0 => p2v is injective only where phys+inner < cap (cap = 16>>shift):
    // the shift drops high bits, so phys+inner must stay in the low (bit_width-shift)
    // bits. That range is exactly the canonical phys set the block places items in.
    #[test]
    fn canonical_range_round_trip() {
        for shift in 1..4u32 {
            let cap = 16u8 >> shift;
            for inner in 0..16u8 {
                let valid = cap.saturating_sub(inner);
                for outer in 0..16u8 {
                    for rot in 0..4u32 {
                        let tr = t(inner, outer, shift, rot);
                        for p in 0..valid {
                            let v = tr.p2v(Nibble::from_u8(p));
                            assert_eq!(tr.v2p(v).as_u8(), p, "inner={inner} outer={outer} shift={shift} rot={rot} phys={p}");
                        }
                    }
                }
            }
        }
    }
}