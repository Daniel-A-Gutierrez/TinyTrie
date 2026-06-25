#![allow(dead_code)]
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
use core::simd::u16x16;
use core::simd::u32x16;
use core::simd::u8x16;

/// Load 16 bytes from `ptr` into a SIMD vector (unaligned), zeroing
/// padding bytes in the range `[valid_end..align_end)` to avoid Miri UB.
///
/// `valid_end` is the first byte after the valid data (e.g., `symbols_offset + count`).
/// `align_end` is the next alignment boundary (typically 8 for a pointer).
/// Bytes in `[valid_end..align_end)` are struct padding and are set to zero.
///
/// Unlike `ptr::read::<u8x16>()` which requires 16-byte alignment,
/// this reads 16 bytes as a `[u8; 16]` (1-byte aligned) and converts.
#[inline]
fn load16_zero_pad(ptr: *const u8, valid_end: usize, align_end: usize) -> u8x16 {
    debug_assert!(valid_end <= align_end);
    debug_assert!(align_end <= 16);
    let mut buf = [0u8; 16];
    // SAFETY: caller guarantees ptr points to at least 16 readable bytes.
    unsafe { core::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), 16) };
    // Zero out padding bytes to avoid Miri UB.
    // These bytes are masked out by the caller's valid_mask anyway,
    // so zeroing them doesn't change the result.
    for i in valid_end..align_end {
        buf[i] = 0;
    }
    u8x16::from(buf)
}

/// Load `n` bytes from `ptr` into a SIMD vector, zero-padding the rest.
///
/// Reads `n` bytes from `ptr` and fills the remaining `16 - n` bytes with
/// zero. This is Miri-safe: it never reads uninitialized bytes beyond the
/// valid range. The zero-padded bytes are masked out by the caller's
/// `valid_mask`, so they don't affect the result.
#[inline]
fn load16_partial(ptr: *const u8, n: usize) -> u8x16 {
    debug_assert!(n > 0 && n < 16);
    let mut buf = [0u8; 16];
    // SAFETY: caller guarantees `ptr` points to at least `n` readable bytes.
    unsafe { core::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), n) };
    u8x16::from(buf)
}

/// Load 16 bytes from `ptr` into a SIMD vector (unaligned).
///
/// Used for contiguous byte arrays (PairVec discriminants) where all 16
/// bytes starting at `ptr` are known to be initialized (e.g., within a
/// full chunk where `offset + 16 <= len` and the allocation is zeroed).
#[inline]
fn load16(ptr: *const u8) -> u8x16 {
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
    if count == 0 {
        return None;
    }
    let valid_mask = ((1u32 << count) - 1) << symbols_offset;
    let valid_end = symbols_offset + count;
    // Padding starts at valid_end, ends at next 8-byte boundary.
    let align_end = (valid_end + 7) & !7;

    let vec = load16_zero_pad(inode_ptr, valid_end, align_end);
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
        // No padding in second load — symbols extend to the end or beyond
        let vec2 = unsafe {
            let mut buf = [0u8; 16];
            core::ptr::copy_nonoverlapping(inode_ptr.add(16), buf.as_mut_ptr(), 16);
            u8x16::from(buf)
        };
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
    if count == 0 {
        return 0;
    }
    let valid_mask = ((1u32 << count) - 1) << symbols_offset;
    let valid_end = symbols_offset + count;
    let align_end = (valid_end + 7) & !7;

    let byte_vec = u8x16::splat(byte);

    let vec = load16_zero_pad(inode_ptr, valid_end, align_end);
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
        let vec2 = unsafe {
            let mut buf = [0u8; 16];
            core::ptr::copy_nonoverlapping(inode_ptr.add(16), buf.as_mut_ptr(), 16);
            u8x16::from(buf)
        };
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
    if len == 0 {
        return None;
    }
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

    // Tail (1..15 bytes) — use partial load to avoid reading uninitialized
    // memory beyond the valid keys (which could be Trie padding in PairVec).
    if offset < len {
        let tail_len = len - offset;
        let valid_mask = (1u32 << tail_len) - 1;
        let vec = load16_partial(unsafe { disc_ptr.add(offset) }, tail_len);
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
    if len == 0 {
        return 0;
    }
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

    // Tail (1..15 bytes) — use partial load to avoid reading uninitialized
    // memory beyond the valid keys (which could be Trie padding in PairVec).
    if offset < len {
        let tail_len = len - offset;
        let valid_mask = (1u32 << tail_len) - 1;
        let vec = load16_partial(unsafe { disc_ptr.add(offset) }, tail_len);
        let lt = vec.simd_lt(byte_vec);
        let mask = lt.to_bitmask() as u32;
        let ge_mask = (!mask) & valid_mask;
        if ge_mask != 0 {
            return offset + ge_mask.trailing_zeros() as usize;
        }
    }

    len
}

/// Compute a 16-bit occupancy mask from a `[u32; 16]` children array.
///
/// Bit N is set if `children[N] != 0`. Used by NibbleTrie iteration to find
/// occupied child slots via `trailing_zeros` / `leading_zeros` instead of
/// linear scanning the 16-slot array.
///
/// Loads the full 64-byte children array into a `u32x16` vector, compares
/// with zero, and extracts a bitmask. The compiler lowers this to the
/// optimal instruction sequence for the target (e.g., two AVX2 loads +
/// movemask, or one AVX-512 load + kmove).
#[inline]
pub fn children_mask(children: &[u32; 16]) -> u16 {
    let vec = u32x16::from(*children);
    let zero = u32x16::splat(0);
    let eq = vec.simd_ne(zero);
    let empty = eq.to_bitmask() as u16; // bit N = 1 if children[N] == 0
    empty // invert: bit N = 1 if children[N] != 0
}

/// Compute a 16-bit occupancy mask from a `[u8; 16]` children array.
///
/// Bit N is set if `children[N] != 0`. Same semantics as `children_mask`
/// but for u8 slot widths — used by `NibbleTrie<T, u8, L>`.
#[inline]
pub fn children_mask_u8(children: &[u8; 16]) -> u16 {
    let vec = u8x16::from(*children);
    let zero = u8x16::splat(0);
    let ne = vec.simd_ne(zero);
    ne.to_bitmask() as u16
}

/// Compute a 16-bit occupancy mask from a `[u16; 16]` children array.
///
/// Bit N is set if `children[N] != 0`. Same semantics as `children_mask`
/// but for u16 slot widths — used by `NibbleTrie<T, u16, L>`.
#[inline]
pub fn children_mask_u16(children: &[u16; 16]) -> u16 {
    let vec = u16x16::from(*children);
    let zero = u16x16::splat(0);
    let ne = vec.simd_ne(zero);
    ne.to_bitmask() as u16
}

/// Compute a 16-bit occupancy mask from a `[u64; 16]` children array.
///
/// Bit N is set if `children[N] != 0`. Same semantics as `children_mask`
/// but for u64 slot widths. Uses two `Simd<u64, 8>` comparisons (128-bit
/// lanes each) to cover all 16 slots, since `Simd<u64, 16>` may require
/// AVX-512 at runtime on some platforms.
#[inline]
pub fn children_mask_u64(children: &[u64; 16]) -> u16 {
    use std::simd::Simd;
    // Two 128-bit chunks: slots 0–7 and 8–15
    let lo = Simd::<u64, 8>::from_slice(&children[0..8]);
    let hi = Simd::<u64, 8>::from_slice(&children[8..16]);
    let zero = Simd::<u64, 8>::splat(0);
    let lo_ne = lo.simd_ne(zero);
    let hi_ne = hi.simd_ne(zero);
    let lo_mask = lo_ne.to_bitmask() as u16;
    let hi_mask = (hi_ne.to_bitmask() as u16) << 8;
    lo_mask | hi_mask
}

#[cfg(test)]
#[path = "tests/simd.rs"]
mod tests;
