//! SIMD-accelerated child lookup using portable SIMD.
//!
//! Replaces scalar linear scan (INode) and binary search (HNode) with
//! branchless vector compare → bitmask → tzcnt on the existing sorted
//! byte arrays. No data layout changes.
//!
//! Uses `core::simd` (portable SIMD), which requires nightly Rust
//! (`#![feature(portable_simd)]`). The compiler lowers these to SSE2/NEON
//! instructions on the target platform automatically — no intrinsics needed.

use core::simd::cmp::{SimdPartialEq, SimdPartialOrd};
use core::simd::u8x16;

/// Load 16 bytes from `ptr` into a SIMD vector (unaligned).
///
/// Unlike `ptr::read::<u8x16>()` which requires 16-byte alignment,
/// this reads 16 bytes as a `[u8; 16]` (1-byte aligned) and converts.
#[inline]
fn load16(ptr: *const u8) -> u8x16 {
    // SAFETY: caller guarantees ptr points to at least 16 readable bytes.
    // We read as [u8; 16] (1-byte aligned) and convert, avoiding
    // the 16-byte alignment requirement of u8x16.
    let mut buf = [0u8; 16];
    unsafe { core::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), 16) };
    u8x16::from(buf)
}

/// Find `byte` in INode symbols using SIMD.
///
/// Loads 16 bytes from the start of the INode struct, masks to the
/// valid symbol positions, and checks for an exact match.
///
/// `symbols_offset` is the byte offset of the `symbols` field within
/// the INode struct (from `offset_of!`). `count` is the number of
/// valid symbols (2..=INLINE).
#[inline]
pub fn inode_find_child(
    inode_ptr: *const u8,
    symbols_offset: usize,
    count: usize,
    byte: u8,
) -> Option<usize> {
    let valid_mask = ((1u32 << count) - 1) << symbols_offset;

    let vec = load16(inode_ptr);
    let byte_vec = u8x16::splat(byte);
    let eq = vec.simd_eq(byte_vec);
    let mask = eq.to_bitmask() as u32;
    let hits = mask & valid_mask;

    if hits != 0 {
        return Some(hits.trailing_zeros() as usize - symbols_offset);
    }

    // Second load if symbols extend past byte 15 (large INLINE values)
    if symbols_offset + count > 16 {
        let first_in_load1 = 16 - symbols_offset;
        let remaining = count - first_in_load1;
        let valid_mask2 = (1u32 << remaining) - 1;
        let vec2 = load16(unsafe { inode_ptr.add(16) });
        let eq2 = vec2.simd_eq(byte_vec);
        let mask2 = eq2.to_bitmask() as u32;
        let hits2 = mask2 & valid_mask2;
        if hits2 != 0 {
            return Some(first_in_load1 + hits2.trailing_zeros() as usize);
        }
    }

    None
}

/// Find first symbol >= `byte` in INode using SIMD (unsigned >=).
///
/// `u8x16::simd_lt` does unsigned comparison, so we get "symbols[i] < byte"
/// directly. Inverting the mask gives "symbols[i] >= byte".
///
/// Returns an index in `0..count`, where `count` means all symbols < byte.
#[inline]
pub fn inode_find_child_lower_bound(
    inode_ptr: *const u8,
    symbols_offset: usize,
    count: usize,
    byte: u8,
) -> usize {
    let valid_mask = ((1u32 << count) - 1) << symbols_offset;

    let byte_vec = u8x16::splat(byte);

    let vec = load16(inode_ptr);
    // simd_lt on u8x16 is unsigned less-than
    let lt = vec.simd_lt(byte_vec);
    let mask = lt.to_bitmask() as u32;

    let ge = (!mask) & valid_mask;
    if ge != 0 {
        return ge.trailing_zeros() as usize - symbols_offset;
    }

    // Second load if symbols extend past byte 15
    if symbols_offset + count > 16 {
        let first_in_load1 = 16 - symbols_offset;
        let remaining = count - first_in_load1;
        let valid_mask2 = (1u32 << remaining) - 1;
        let vec2 = load16(unsafe { inode_ptr.add(16) });
        let lt2 = vec2.simd_lt(byte_vec);
        let mask2 = lt2.to_bitmask() as u32;
        let ge2 = (!mask2) & valid_mask2;
        if ge2 != 0 {
            return first_in_load1 + ge2.trailing_zeros() as usize;
        }
    }

    count
}

/// Find `byte` in HNode discriminants using SIMD.
///
/// Processes the discriminants array in 16-byte chunks.
/// Returns `None` if not found.
#[inline]
pub fn hnode_find_child(disc_ptr: *const u8, len: usize, byte: u8) -> Option<usize> {
    let byte_vec = u8x16::splat(byte);
    let mut offset = 0usize;

    // Full 16-byte chunks
    while offset + 16 <= len {
        let vec = load16(unsafe { disc_ptr.add(offset) });
        let eq = vec.simd_eq(byte_vec);
        let mask = eq.to_bitmask() as u32;
        if mask != 0 {
            return Some(offset + mask.trailing_zeros() as usize);
        }
        offset += 16;
    }

    // Tail (1..15 bytes)
    if offset < len {
        let tail_len = len - offset;
        let valid_mask = (1u32 << tail_len) - 1;
        let vec = load16(unsafe { disc_ptr.add(offset) });
        let eq = vec.simd_eq(byte_vec);
        let mask = eq.to_bitmask() as u32;
        let hits = mask & valid_mask;
        if hits != 0 {
            return Some(offset + hits.trailing_zeros() as usize);
        }
    }

    None
}

/// Find first discriminant >= `byte` in HNode using SIMD.
///
/// `u8x16::simd_lt` does unsigned comparison directly — no XOR bias needed.
/// Processes chunks sequentially, stopping at the first chunk with a match.
/// Since discriminants are sorted, the first `>=` match in chunk order
/// is the global lower bound.
///
/// Returns an index in `0..len`, where `len` means all discriminants < byte.
#[inline]
pub fn hnode_find_child_lower_bound(disc_ptr: *const u8, len: usize, byte: u8) -> usize {
    let byte_vec = u8x16::splat(byte);
    let mut offset = 0usize;

    // Full 16-byte chunks
    while offset + 16 <= len {
        let vec = load16(unsafe { disc_ptr.add(offset) });
        let lt = vec.simd_lt(byte_vec);
        let mask = lt.to_bitmask() as u32;

        if mask != 0xFFFF {
            let ge_mask = (!mask) & 0xFFFF;
            return offset + ge_mask.trailing_zeros() as usize;
        }
        offset += 16;
    }

    // Tail (1..15 bytes)
    if offset < len {
        let tail_len = len - offset;
        let valid_mask = (1u32 << tail_len) - 1;
        let vec = load16(unsafe { disc_ptr.add(offset) });
        let lt = vec.simd_lt(byte_vec);
        let mask = lt.to_bitmask() as u32;
        let ge_mask = (!mask) & valid_mask;
        if ge_mask != 0 {
            return offset + ge_mask.trailing_zeros() as usize;
        }
    }

    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_inode_buf(symbols_offset: usize, symbols: &[u8]) -> Vec<u8> {
        let total = symbols_offset + symbols.len();
        let mut buf = vec![0u8; total.max(16).max(32)];
        buf[0] = symbols.len() as u8;
        for (i, &s) in symbols.iter().enumerate() {
            buf[symbols_offset + i] = s;
        }
        buf
    }

    fn make_disc_buf(discs: &[u8]) -> Vec<u8> {
        let len = discs.len();
        let mut buf = vec![0u8; len.max(16) + 32];
        buf[..len].copy_from_slice(discs);
        buf
    }

    #[test]
    fn test_inode_find_child_basic() {
        let symbols_offset = 2;
        let symbols = [10u8, 20, 30, 40, 50, 60];
        let buf = make_inode_buf(symbols_offset, &symbols);

        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 10),
            Some(0)
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 30),
            Some(2)
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 60),
            Some(5)
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 15),
            None
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 5),
            None
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 6, 65),
            None
        );
    }

    #[test]
    fn test_inode_find_child_min_count() {
        let symbols_offset = 2;
        let buf = make_inode_buf(symbols_offset, &[100, 200]);
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 2, 100),
            Some(0)
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 2, 200),
            Some(1)
        );
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 2, 150),
            None
        );
    }

    #[test]
    fn test_inode_find_child_boundary_bytes() {
        let symbols_offset = 2;
        for &byte in &[0x00, 0x7F, 0x80, 0xFF] {
            let buf = make_inode_buf(symbols_offset, &[byte, byte.wrapping_add(1)]);
            assert_eq!(
                inode_find_child(buf.as_ptr(), symbols_offset, 2, byte),
                Some(0)
            );
        }
    }

    #[test]
    fn test_inode_find_child_lower_bound_basic() {
        let symbols_offset = 2;
        let symbols = [10u8, 20, 30, 40, 50, 60];
        let buf = make_inode_buf(symbols_offset, &symbols);

        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 10),
            0
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 30),
            2
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 60),
            5
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 15),
            1
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 5),
            0
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 65),
            6
        );
    }

    #[test]
    fn test_inode_find_child_lower_bound_boundary_bytes() {
        let symbols_offset = 2;
        let buf = make_inode_buf(symbols_offset, &[0x00, 0x7F, 0x80, 0xFF]);

        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x00),
            0
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x7F),
            1
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x80),
            2
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0xFF),
            3
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x40),
            1
        );
        assert_eq!(
            inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x90),
            3
        );
    }

    #[test]
    fn test_hnode_find_child_basic() {
        let discs: Vec<u8> = (10..=70).step_by(10).collect();
        let buf = make_disc_buf(&discs);

        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 10), Some(0));
        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 40), Some(3));
        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 70), Some(6));
        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 15), None);
        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 5), None);
        assert_eq!(hnode_find_child(buf.as_ptr(), 7, 75), None);
    }

    #[test]
    fn test_hnode_find_child_chunk_boundaries() {
        // len=16 (exactly one chunk)
        let discs16: Vec<u8> = (0..16).map(|i| i * 10).collect();
        let buf16 = make_disc_buf(&discs16);
        assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 0), Some(0));
        assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 150), Some(15));
        assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 85), None);

        // len=17 (one chunk + 1-byte tail)
        let discs17: Vec<u8> = (0..17).map(|i| i * 10).collect();
        let buf17 = make_disc_buf(&discs17);
        assert_eq!(hnode_find_child(buf17.as_ptr(), 17, 160), Some(16));

        // len=31 (one chunk + 15-byte tail)
        let discs31: Vec<u8> = (0..31).map(|i| i * 8).collect();
        let buf31 = make_disc_buf(&discs31);
        assert_eq!(hnode_find_child(buf31.as_ptr(), 31, 0), Some(0));
        assert_eq!(hnode_find_child(buf31.as_ptr(), 31, 240), Some(30));
    }

    #[test]
    fn test_hnode_find_child_boundary_bytes() {
        let discs = [0x00, 0x40, 0x7F, 0x80, 0xC0, 0xFF];
        let buf = make_disc_buf(&discs);

        for &byte in &discs {
            assert_eq!(
                hnode_find_child(buf.as_ptr(), discs.len(), byte),
                Some(discs.iter().position(|&d| d == byte).unwrap())
            );
        }
    }

    #[test]
    fn test_hnode_find_child_lower_bound_basic() {
        let discs: Vec<u8> = (10..=70).step_by(10).collect();
        let buf = make_disc_buf(&discs);

        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 10), 0);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 40), 3);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 70), 6);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 15), 1);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 25), 2);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 5), 0);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 75), 7);
    }

    #[test]
    fn test_hnode_find_child_lower_bound_boundary_bytes() {
        let discs = [0x00, 0x40, 0x7F, 0x80, 0xC0, 0xFF];
        let buf = make_disc_buf(&discs);

        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x40), 1);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x7F), 2);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x80), 3);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x90), 4);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0xFF), 5);
    }

    #[test]
    fn test_hnode_large_discriminants() {
        // 32 discriminants (2 chunks, no tail)
        let discs: Vec<u8> = (0..32).collect();
        let buf = make_disc_buf(&discs);
        for i in 0u8..32 {
            assert_eq!(hnode_find_child(buf.as_ptr(), 32, i), Some(i as usize));
            assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 32, i), i as usize);
        }
        assert_eq!(hnode_find_child(buf.as_ptr(), 32, 32), None);
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 32, 32), 32);
    }
}