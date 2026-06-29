//! Packed key storage for variable-length B+ tree nodes.
//!
//! `PackedKeySlots<L, N>` stores up to N keys in a dense packed layout:
//! - `lens[i]`: total key length (0 = empty slot), stored as type `L`.
//! - `packed`: all key bytes contiguously, no padding. Key `i` starts at
//!   `sum(lens[0..i])` and spans `lens[i]` bytes. Stored in a SmallVec that
//!   keeps the first `PACKED_INLINE` bytes on-stack before spilling to heap.
//!
//! The sequential scan walks `packed` with a running byte offset, comparing
//! each key against the needle in one contiguous memory region.

use smallvec::SmallVec;

/// Inline capacity for the packed key buffer (bytes kept on-stack before heap).
pub const PACKED_INLINE: usize = 64;

// ---------------------------------------------------------------------------
// LengthType trait
// ---------------------------------------------------------------------------

/// Trait for types that can store key lengths in `PackedKeySlots`.
///
/// Implemented for `u8`, `u16`, `u32`, `u64`, and `usize`. The choice of `L`
/// determines the maximum key length the tree can store — e.g. `u8` limits
/// keys to 255 bytes, `u16` to 65535 bytes.
pub trait LengthType: Copy + Clone + Default + Ord + std::fmt::Debug + 'static {
    /// Convert to `usize`.
    fn as_usize(self) -> usize;
    /// Maximum representable value (e.g. 255 for `u8`).
    fn max() -> usize;
    /// The zero value (represents an empty slot in `lens`).
    fn zero() -> Self;
    /// Convert a `usize` to `Self`. Panics if `n` exceeds `Self::max()`.
    fn usize_as(n: usize) -> Self;
}

macro_rules! impl_length_type {
    ($($ty:ty),* $(,)?) => {
        $(
            impl LengthType for $ty {
                #[inline] fn as_usize(self) -> usize { self as usize }
                #[inline] fn max() -> usize { <$ty>::MAX as usize }
                #[inline] fn zero() -> Self { 0 }
                #[inline] fn usize_as(n: usize) -> Self {
                    debug_assert!(n <= <$ty>::MAX as usize, "key length {} exceeds {}::MAX", n, stringify!($ty));
                    n as $ty
                }
            }
        )*
    };
}

impl_length_type!(u8, u16, u32, u64, usize);

// ---------------------------------------------------------------------------
// PackedKeySlots
// ---------------------------------------------------------------------------

/// Packed key storage for up to N variable-length keys.
///
/// Keys are stored densely — a 3-byte key takes 3 bytes, a 50-byte key takes
/// 50 bytes, with no padding. The first `PACKED_INLINE` bytes of packed data
/// are kept inline in the struct; overflow goes to the heap.
///
/// The type parameter `L` determines how key lengths are stored. `u8` limits
/// keys to 255 bytes, `u16` to 65535, etc.
pub struct PackedKeySlots<L: LengthType, const N: usize> {
    /// Number of occupied key slots.
    len: u8,
    /// Key length per slot. `L::zero()` = empty.
    lens: [L; N],
    /// Dense key data. Key `i` starts at `sum(lens[0..i])`.
    packed: SmallVec<[u8; PACKED_INLINE]>,
}

impl<L: LengthType, const N: usize> PackedKeySlots<L, N> {
    /// Create an empty `PackedKeySlots`.
    pub fn new() -> Self {
        Self {
            len: 0,
            lens: [L::zero(); N],
            packed: SmallVec::new(),
        }
    }

    /// Number of occupied key slots.
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Is the array at capacity?
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len as usize == N
    }

    /// Reserve capacity for `additional` bytes in the packed buffer.
    pub fn reserve(&mut self, additional: usize) {
        self.packed.reserve(additional);
    }

    /// Total bytes currently used in the packed buffer.
    #[inline]
    pub fn packed_len(&self) -> usize {
        self.packed.len()
    }

    /// Key length at slot `i`.
    #[inline]
    pub fn key_len(&self, i: usize) -> usize {
        debug_assert!(i < self.len as usize);
        self.lens[i].as_usize()
    }

    /// Compute byte offset into `packed` for key `i`.
    ///
    /// Sum of `lens[0..i]`. O(i).
    pub fn packed_offset_up_to(&self, i: usize) -> usize {
        let mut off = 0;
        for j in 0..i {
            off += self.lens[j].as_usize();
        }
        off
    }

    /// Return a slice referencing the full key at slot `i`.
    #[inline]
    pub fn key_slice(&self, i: usize) -> &[u8] {
        debug_assert!(i < self.len as usize);
        let start = self.packed_offset_up_to(i);
        let klen = self.lens[i].as_usize();
        &self.packed[start..start + klen]
    }

    /// Return a slice referencing the full key at slot `i`, using a
    /// pre-computed packed offset. Also returns the offset for slot `i+1`.
    #[inline]
    pub fn key_slice_with_offset(&self, i: usize, packed_off: usize) -> (&[u8], usize) {
        debug_assert!(i < self.len as usize);
        let klen = self.lens[i].as_usize();
        (&self.packed[packed_off..packed_off + klen], packed_off + klen)
    }

    /// Reconstruct the full key at slot `i` as a `Vec<u8>`.
    pub fn get_key(&self, i: usize) -> Vec<u8> {
        self.key_slice(i).to_vec()
    }

    /// Insert `key` at position `pos`, shifting later keys right.
    ///
    /// Panics if the array is full, `pos > len`, or the key length exceeds
    /// `<L as LengthType>::max()`.
    pub fn insert_at(&mut self, pos: usize, key: &[u8]) {
        debug_assert!(!self.is_full(), "PackedKeySlots::insert_at: array is full");
        debug_assert!(
            pos <= self.len as usize,
            "PackedKeySlots::insert_at: pos out of bounds"
        );
        assert!(
            key.len() <= <L as LengthType>::max(),
            "PackedKeySlots::insert_at: key length {} exceeds maximum {}",
            key.len(),
            <L as LengthType>::max()
        );
        let l = self.len as usize;
        let klen = key.len();

        // Compute byte offset before shifting lens (lens[0..pos] is still correct)
        let packed_off = self.packed_offset_up_to(pos);

        // Shift lens right
        if pos < l {
            self.lens.copy_within(pos..l, pos + 1);
        }
        self.lens[pos] = L::usize_as(klen);

        // Shift packed data right by klen bytes
        let old_len = self.packed.len();
        self.packed.resize(old_len + klen, 0);
        if packed_off < old_len {
            self.packed.as_mut_slice().copy_within(packed_off..old_len, packed_off + klen);
        }
        self.packed[packed_off..packed_off + klen].copy_from_slice(key);

        self.len += 1;
    }

    /// Remove key at position `pos`, shifting later keys left.
    ///
    /// Returns the removed key as a `Vec<u8>`.
    pub fn remove_at(&mut self, pos: usize) -> Vec<u8> {
        debug_assert!(
            pos < self.len as usize,
            "PackedKeySlots::remove_at: index out of bounds"
        );
        let l = self.len as usize;
        let klen = self.lens[pos].as_usize();

        // Compute packed offset before modifying lens
        let packed_off = self.packed_offset_up_to(pos);

        // Extract key
        let key = self.packed[packed_off..packed_off + klen].to_vec();

        // Shift packed data left
        let packed_end = self.packed.len();
        if packed_off + klen < packed_end {
            self.packed.as_mut_slice().copy_within(packed_off + klen..packed_end, packed_off);
        }
        self.packed.truncate(packed_end - klen);

        // Shift lens left
        if pos + 1 < l {
            self.lens.copy_within(pos + 1..l, pos);
        }
        self.lens[l - 1] = L::zero();

        self.len -= 1;
        key
    }

    /// Replace the key at position `pos` with `key`.
    ///
    /// Panics if the key length exceeds `<L as LengthType>::max()`.
    pub fn update_at(&mut self, pos: usize, key: &[u8]) {
        debug_assert!(pos < self.len as usize, "PackedKeySlots::update_at: index out of bounds");
        assert!(
            key.len() <= <L as LengthType>::max(),
            "PackedKeySlots::update_at: key length {} exceeds maximum {}",
            key.len(),
            <L as LengthType>::max()
        );
        let new_klen = key.len();
        let old_klen = self.lens[pos].as_usize();

        let packed_off = self.packed_offset_up_to(pos);

        if new_klen == old_klen {
            // Same size — write in place
            self.packed[packed_off..packed_off + new_klen].copy_from_slice(key);
        } else if new_klen > old_klen {
            // Grew — shift right
            let delta = new_klen - old_klen;
            let old_packed_len = self.packed.len();
            self.packed.resize(old_packed_len + delta, 0);
            self.packed.as_mut_slice()
                .copy_within(packed_off + old_klen..old_packed_len, packed_off + new_klen);
            self.packed[packed_off..packed_off + new_klen].copy_from_slice(key);
        } else {
            // Shrunk — shift left
            let delta = old_klen - new_klen;
            let packed_end = self.packed.len();
            self.packed.as_mut_slice()
                .copy_within(packed_off + old_klen..packed_end, packed_off + new_klen);
            self.packed.truncate(packed_end - delta);
            self.packed[packed_off..packed_off + new_klen].copy_from_slice(key);
        }

        self.lens[pos] = L::usize_as(new_klen);
    }

    /// Append `key` to the end of the array.
    ///
    /// Panics if the array is full or the key length exceeds `<L as LengthType>::max()`.
    pub fn push(&mut self, key: &[u8]) {
        self.insert_at(self.len as usize, key);
    }

    /// Set `len` to `new_len` without dropping elements.
    ///
    /// The caller must ensure that truncated elements have been moved elsewhere.
    /// Truncates `packed` to the byte extent of the remaining keys.
    pub fn truncate(&mut self, new_len: u8) {
        debug_assert!(
            new_len as usize <= N,
            "PackedKeySlots::truncate: new_len exceeds capacity"
        );
        let new_packed_len = self.packed_offset_up_to(new_len as usize);
        self.packed.truncate(new_packed_len);
        self.len = new_len;
    }

    /// Move elements `[from..len)` into `dst`, starting at `dst` index 0.
    ///
    /// After this call:
    /// - `dst` has `self.len - from` elements in slots `[0..self.len - from)`.
    /// - `self` is truncated to `from`.
    pub fn drain_into(&mut self, from: usize, dst: &mut Self) {
        let count = self.len as usize - from;
        debug_assert!(
            from < self.len as usize,
            "PackedKeySlots::drain_into: empty drain"
        );
        debug_assert!(
            dst.len as usize + count <= N,
            "PackedKeySlots::drain_into: dst overflow"
        );

        // Compute source packed byte range for keys [from..len)
        let src_packed_start = self.packed_offset_up_to(from);
        let src_packed_end = self.packed.len();

        // Move lens
        for i in 0..count {
            dst.lens[dst.len as usize + i] = self.lens[from + i];
        }

        // Move packed data
        dst.packed.extend_from_slice(&self.packed[src_packed_start..src_packed_end]);

        // Clear moved lens in src
        for i in from..self.len as usize {
            self.lens[i] = L::zero();
        }
        // Remove moved packed range from src
        self.packed.drain(src_packed_start..src_packed_end);

        dst.len += count as u8;
        self.len = from as u8;
    }

    /// Move `self[from..len)` to **dst's front** (prepend), shifting dst's
    /// existing elements right.
    pub fn drain_into_front(&mut self, from: usize, dst: &mut Self) {
        let count = self.len as usize - from;
        debug_assert!(
            from < self.len as usize,
            "PackedKeySlots::drain_into_front: empty drain"
        );
        debug_assert!(
            dst.len as usize + count <= N,
            "PackedKeySlots::drain_into_front: dst overflow"
        );

        let dl = dst.len as usize;

        // Compute source packed byte range
        let src_packed_start = self.packed_offset_up_to(from);
        let src_packed_end = self.packed.len();

        // Shift dst's existing lens right by `count`
        if dl > 0 {
            dst.lens.copy_within(0..dl, count);
        }

        // Copy self's [from..len) to dst's front
        for i in 0..count {
            dst.lens[i] = self.lens[from + i];
        }

        // Prepend self's packed data to dst's front.
        // Extend dst (appends src_data after dst's existing data),
        // then rotate so src_data comes first.
        let src_packed_bytes = src_packed_end - src_packed_start;
        dst.packed.extend_from_slice(&self.packed[src_packed_start..src_packed_end]);
        dst.packed.as_mut_slice().rotate_right(src_packed_bytes);

        // Clear moved data in src
        for i in from..self.len as usize {
            self.lens[i] = L::zero();
        }
        self.packed.drain(src_packed_start..src_packed_end);

        dst.len += count as u8;
        self.len = from as u8;
    }

    /// Move `self[0..count)` to **dst's end** (append), then shift self's
    /// remaining elements `[count..len)` left to the front.
    pub fn drain_front_into(&mut self, count: usize, dst: &mut Self) {
        debug_assert!(
            count <= self.len as usize,
            "PackedKeySlots::drain_front_into: count out of bounds"
        );
        debug_assert!(
            dst.len as usize + count <= N,
            "PackedKeySlots::drain_front_into: dst overflow"
        );
        let l = self.len as usize;
        let dl = dst.len as usize;

        // Compute source packed byte range for keys [0..count)
        let mut src_packed_end = 0usize;
        for i in 0..count {
            src_packed_end += self.lens[i].as_usize();
        }

        // Append self's [0..count) lens to dst's end
        for i in 0..count {
            dst.lens[dl + i] = self.lens[i];
        }

        // Append self's [0..count) packed data to dst's end
        dst.packed.extend_from_slice(&self.packed[0..src_packed_end]);

        // Shift src's remaining keys [count..l) left
        if count < l {
            self.lens.copy_within(count..l, 0);
        }

        // Clear vacated lens in src
        for i in (l - count)..l {
            self.lens[i] = L::zero();
        }

        // Shift src's remaining packed data left
        let remaining = self.packed.len();
        if src_packed_end < remaining {
            self.packed.as_mut_slice().copy_within(src_packed_end..remaining, 0);
        }
        self.packed.truncate(remaining - src_packed_end);

        dst.len += count as u8;
        self.len -= count as u8;
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// First index where `keys[i] >= needle` (lower bound).
    ///
    /// Walks `packed` with a running byte offset, comparing each key
    /// against the needle.
    pub fn find_position(&self, needle: &[u8]) -> usize {
        self.find_position_with_offset(needle).0
    }

    /// First index where `keys[i] >= needle`, plus the packed byte offset
    /// of that position (or the total packed length if the needle exceeds
    /// all keys).
    ///
    /// The offset can be reused for subsequent `eq_key_with_offset` or
    /// `key_slice_with_offset` calls, avoiding a redundant O(i) scan.
    #[inline]
    pub fn find_position_with_offset(&self, needle: &[u8]) -> (usize, usize) {
        let l = self.len as usize;
        let mut off = 0usize;
        for i in 0..l {
            let klen = self.lens[i].as_usize();
            let key = &self.packed[off..off + klen];
            if needle <= key {
                return (i, off);
            }
            off += klen;
        }
        (l, off)
    }

    /// First index where `keys[i] > needle` (upper bound).
    pub fn find_upper_bound(&self, needle: &[u8]) -> usize {
        let l = self.len as usize;
        let mut off = 0usize;
        for i in 0..l {
            let klen = self.lens[i].as_usize();
            let key = &self.packed[off..off + klen];
            if needle < key {
                return i;
            }
            off += klen;
        }
        l
    }

    /// Check if the key at position `i` is equal to `needle`.
    pub fn eq_key(&self, i: usize, needle: &[u8]) -> bool {
        debug_assert!(i < self.len as usize);
        let klen = self.lens[i].as_usize();
        if klen != needle.len() {
            return false;
        }
        let start = self.packed_offset_up_to(i);
        needle == &self.packed[start..start + klen]
    }

    /// Check if the key at position `i` with pre-computed packed offset
    /// `off` is equal to `needle`. O(1) — avoids the O(i) offset scan.
    #[inline]
    pub fn eq_key_with_offset(&self, i: usize, off: usize, needle: &[u8]) -> bool {
        debug_assert!(i < self.len as usize);
        let klen = self.lens[i].as_usize();
        if klen != needle.len() {
            return false;
        }
        needle == &self.packed[off..off + klen]
    }
}

impl<L: LengthType, const N: usize> Clone for PackedKeySlots<L, N> {
    fn clone(&self) -> Self {
        Self {
            len: self.len,
            lens: self.lens,
            packed: self.packed.clone(),
        }
    }
}

impl<L: LengthType, const N: usize> std::fmt::Debug for PackedKeySlots<L, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys = Vec::new();
        for i in 0..self.len as usize {
            keys.push(self.get_key(i));
        }
        f.debug_struct("PackedKeySlots")
            .field("len", &self.len)
            .field("keys", &keys)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_scan() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"hello");
        slots.insert_at(1, b"world");
        slots.insert_at(2, b"abc");
        assert_eq!(slots.len(), 3);
        assert_eq!(slots.key_len(0), 5);
        assert_eq!(slots.key_len(1), 5);
        assert_eq!(slots.key_len(2), 3);

        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"hello"), 0);
        assert_eq!(slots.find_position(b"mid"), 1);
        assert_eq!(slots.find_position(b"world"), 1);
        assert_eq!(slots.find_position(b"zzz"), 3);
    }

    #[test]
    fn test_sorted_scan() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"abc");
        slots.insert_at(1, b"hello");
        slots.insert_at(2, b"world");

        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"abc"), 0);
        assert_eq!(slots.find_position(b"bcd"), 1)
        ;
        assert_eq!(slots.find_position(b"hello"), 1);
        assert_eq!(slots.find_position(b"mid"), 2);
        assert_eq!(slots.find_position(b"world"), 2);
        assert_eq!(slots.find_position(b"zzz"), 3);
    }

    #[test]
    fn test_insert_long_key() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        let long_key = b"this_is_a_very_long_key_that_overflows";
        slots.insert_at(0, b"abc");
        slots.insert_at(1, long_key);
        slots.insert_at(2, b"xyz");

        assert_eq!(slots.len(), 3);
        assert_eq!(slots.key_len(0), 3);
        assert_eq!(slots.key_len(1), long_key.len());
        assert_eq!(slots.key_len(2), 3);

        assert_eq!(slots.find_position(b"abc"), 0);
        assert_eq!(slots.find_position(long_key), 1);
        assert_eq!(slots.find_position(b"xyz"), 2);
        assert_eq!(slots.find_position(b"mid"), 1);
    }

    #[test]
    fn test_packed_density() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        // 3-byte keys should take exactly 3 bytes each in packed
        slots.insert_at(0, b"aaa");
        slots.insert_at(1, b"bbb");
        slots.insert_at(2, b"ccc");
        assert_eq!(slots.packed.len(), 9); // 3 keys x 3 bytes each
    }

    #[test]
    fn test_remove() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"alpha");
        slots.insert_at(1, b"beta");
        slots.insert_at(2, b"gamma");

        let removed = slots.remove_at(1);
        assert_eq!(&removed, b"beta");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots.find_position(b"alpha"), 0);
        assert_eq!(slots.find_position(b"gamma"), 1);
    }

    #[test]
    fn test_remove_long_key() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        let long_key = b"this_is_a_very_long_key_that_overflows";
        slots.insert_at(0, b"abc");
        slots.insert_at(1, long_key);
        slots.insert_at(2, b"xyz");

        let removed = slots.remove_at(1);
        assert_eq!(&removed, long_key);
        assert_eq!(slots.len(), 2);

        assert_eq!(slots.find_position(b"abc"), 0);
        assert_eq!(slots.find_position(b"xyz"), 1);
    }

    #[test]
    fn test_drain_into() {
        let mut src: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        let mut dst: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        let long_key = b"overflow_key_here_for_testing";
        src.insert_at(0, b"aaa");
        src.insert_at(1, long_key);
        src.insert_at(2, b"ccc");
        src.insert_at(3, b"ddd");

        src.drain_into(2, &mut dst);
        assert_eq!(src.len(), 2);
        assert_eq!(dst.len(), 2);

        assert_eq!(src.get_key(0), b"aaa");
        assert_eq!(src.get_key(1), long_key);
        assert_eq!(dst.get_key(0), b"ccc");
        assert_eq!(dst.get_key(1), b"ddd");
    }

    #[test]
    fn test_eq_key() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        let long_key = b"this_is_a_long_key_for_overflow";
        slots.insert_at(0, b"abc");
        slots.insert_at(1, long_key);

        assert!(slots.eq_key(0, b"abc"));
        assert!(!slots.eq_key(0, b"abcd"));
        assert!(slots.eq_key(1, long_key));
        assert!(!slots.eq_key(1, b"this_is_a_long_key_for_overflo"));
    }

    #[test]
    fn test_insert_at_front() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"ccc");
        slots.insert_at(0, b"aaa");
        slots.insert_at(1, b"bbb");

        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"bbb"), 1);
        assert_eq!(slots.find_position(b"ccc"), 2);
    }

    #[test]
    fn test_find_upper_bound() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"aaa");
        slots.insert_at(1, b"ccc");
        slots.insert_at(2, b"eee");

        assert_eq!(slots.find_upper_bound(b"aaa"), 1);
        assert_eq!(slots.find_upper_bound(b"bbb"), 1);
        assert_eq!(slots.find_upper_bound(b"ccc"), 2);
        assert_eq!(slots.find_upper_bound(b"zzz"), 3);
    }

    #[test]
    fn test_all_long_keys() {
        let mut slots: PackedKeySlots<u8, 16> = PackedKeySlots::new();
        let keys: &[&[u8]] = &[
            b"alpha_bravo_charlie",
            b"alpha_bravo_delta",
            b"echo_foxtrot",
            b"golf_hotel_india",
        ];
        for (i, k) in keys.iter().enumerate() {
            slots.insert_at(i, k);
        }
        assert_eq!(slots.len(), 4);

        for (i, k) in keys.iter().enumerate() {
            assert_eq!(
                slots.find_position(k),
                i,
                "find_position for key {}",
                i
            );
            assert!(slots.eq_key(i, k), "eq_key for key {}", i);
        }
    }

    #[test]
    fn test_mixed_key_lengths() {
        let mut slots: PackedKeySlots<u8, 16> = PackedKeySlots::new();
        slots.insert_at(0, b"another_very_long_key_here");
        slots.insert_at(1, b"mid");
        slots.insert_at(2, b"short");
        slots.insert_at(3, b"this_is_a_long_overflowing_key");

        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"another_very_long_key_here"), 0);
        assert_eq!(slots.find_position(b"mid"), 1);
        assert_eq!(slots.find_position(b"mbe"), 1);
        assert_eq!(slots.find_position(b"short"), 2);
        assert_eq!(slots.find_position(b"this_is_a_long_overflowing_key"), 3);
        assert_eq!(slots.find_position(b"zzz"), 4);
    }

    #[test]
    fn test_find_position_edge_cases() {
        let mut slots: PackedKeySlots<u8, 16> = PackedKeySlots::new();
        assert_eq!(slots.find_position(b"anything"), 0);

        slots.insert_at(0, b"hello");
        assert_eq!(slots.find_position(b"hello"), 0);
        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"zzz"), 1);

        let long = b"very_long_key_that_overflows_the_inline_limit";
        slots.remove_at(0);
        slots.insert_at(0, long);
        assert_eq!(slots.find_position(long), 0);
        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"zzz"), 1);
    }

    #[test]
    fn test_prefix_matching() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"abc");
        slots.insert_at(1, b"abcd");

        assert_eq!(slots.find_position(b"abc"), 0);
        assert_eq!(slots.find_position(b"abcd"), 1);
        assert_eq!(slots.find_position(b"ab"), 0);
        assert_eq!(slots.find_position(b"abcde"), 2);
    }

    #[test]
    fn test_update_at() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"alpha");
        slots.insert_at(1, b"beta");
        slots.insert_at(2, b"gamma");

        // Replace inline key with inline key (same size)
        slots.update_at(1, b"delta");
        assert_eq!(slots.get_key(1), b"delta");
        assert_eq!(slots.find_position(b"delta"), 1);

        // Replace short key with long key
        let long = b"this_is_a_very_long_key";
        slots.update_at(0, long);
        assert_eq!(slots.get_key(0), long);
        assert_eq!(slots.find_position(long), 0);

        // Replace long key with short key
        slots.update_at(0, b"aaa");
        assert_eq!(slots.get_key(0), b"aaa");
        assert_eq!(slots.find_position(b"aaa"), 0);
        assert_eq!(slots.find_position(b"beta"), 1);
    }

    #[test]
    fn test_update_at_shrink() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"long_key_here");
        slots.insert_at(1, b"mid");

        slots.update_at(0, b"a");
        assert_eq!(slots.get_key(0), b"a");
        assert_eq!(slots.get_key(1), b"mid");
        assert_eq!(slots.packed.len(), 4); // 1 + 3
    }

    #[test]
    fn test_update_at_grow() {
        let mut slots: PackedKeySlots<u8, 8> = PackedKeySlots::new();
        slots.insert_at(0, b"a");
        slots.insert_at(1, b"mid");

        slots.update_at(0, b"long_key_here");
        assert_eq!(slots.get_key(0), b"long_key_here");
        assert_eq!(slots.get_key(1), b"mid");
    }
}