//! PairVec: A dual-section allocation for sorted (key, value) pairs.
//!
//! Replaces HNode in the trie. Stores sorted byte discriminants (keys)
//! and child trie nodes (values) contiguously with spare capacity for
//! amortized O(1) insertion — no full reallocation on every insert.
//!
//! Data layout: `[keys: u8 × capacity | padding | values: Trie × capacity]`
//!
//! Only the first `len` entries in each section are valid. The spare
//! capacity beyond `len` absorbs insertions via in-place shifts,
//! avoiding allocation until `len == capacity`.

use crate::{align_up, PrefixLen, Trie};
use std::alloc::{self, Layout};
use std::mem::size_of;

/// A PairVec node in the trie — replaces HNode.
///
/// Stores sorted byte discriminants and child trie nodes in a single
/// allocation with spare capacity for amortized growth.
///
/// The `len` field doubles as the node tag: for PairVec nodes,
/// `len > INLINE` (and `len` IS the child count).
///
/// **Layout in 16-byte union slot (PREFIX=u8):**
/// ```text
/// [0]     len (u8) = tag
/// [1]     capacity (u8)
/// [2]     prefix_len (PREFIX)
/// [3..8]  unused padding
/// [8..16] ptr (*mut u8)
/// ```
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct PairVec<const INLINE: usize, PREFIX: PrefixLen> {
    pub len: u8,
    pub capacity: u8,
    pub prefix_len: PREFIX,
    pub ptr: *mut u8,
}

unsafe impl<const INLINE: usize, PREFIX: PrefixLen> Send for PairVec<INLINE, PREFIX> {}
unsafe impl<const INLINE: usize, PREFIX: PrefixLen> Sync for PairVec<INLINE, PREFIX> {}

impl<const INLINE: usize, PREFIX: PrefixLen> PairVec<INLINE, PREFIX> {
    /// Offset from `ptr` to the start of the values section.
    ///
    /// Keys occupy `capacity` bytes at the start, then padding to
    /// align the values section to `align_of::<Trie>()`.
    #[inline]
    pub fn values_offset(capacity: usize) -> usize {
        align_up(capacity, std::mem::align_of::<Trie<INLINE, PREFIX>>())
    }

    /// Total allocation size for a given capacity.
    #[inline]
    fn alloc_size(capacity: usize) -> usize {
        let values_off = Self::values_offset(capacity);
        values_off + capacity * size_of::<Trie<INLINE, PREFIX>>()
    }

    /// Allocation layout for a given capacity.
    fn layout(capacity: usize) -> Layout {
        Layout::from_size_align(
            Self::alloc_size(capacity),
            std::mem::align_of::<Trie<INLINE, PREFIX>>(),
        )
        .unwrap()
    }

    /// Create a PairVec with zeroed padding.
    ///
    /// All bytes including `#[repr(C)]` padding are initialized,
    /// ensuring Miri-safe reads of the struct.
    pub(crate) fn new(len: u8, capacity: u8, prefix_len: PREFIX, ptr: *mut u8) -> Self {
        let mut pv: Self = unsafe { std::mem::zeroed() };
        pv.len = len;
        pv.capacity = capacity;
        pv.prefix_len = prefix_len;
        pv.ptr = ptr;
        pv
    }

    /// Read the discriminants slice (keys).
    ///
    /// # Safety
    /// The PairVec must be valid and its data allocation intact.
    #[inline]
    pub unsafe fn keys(&self) -> &[u8] {
        // SAFETY: caller guarantees the PairVec is valid and its data
        // allocation is intact.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
    }

    /// Read the values (children) slice.
    ///
    /// # Safety
    /// The PairVec must be valid and its data allocation intact.
    #[inline]
    pub unsafe fn values(&self) -> &[Trie<INLINE, PREFIX>] {
        // SAFETY: caller guarantees the PairVec is valid and its data
        // allocation is intact. values_offset gives an aligned offset.
        unsafe {
            let off = Self::values_offset(self.capacity as usize);
            let ptr = self.ptr.add(off) as *const Trie<INLINE, PREFIX>;
            std::slice::from_raw_parts(ptr, self.len as usize)
        }
    }

    /// Find a child by its discriminant byte. Returns the index or `None`.
    #[inline]
    pub fn find_child(&self, byte: u8) -> Option<usize> {
        crate::simd::hnode_find_child(self.ptr, self.len as usize, byte)
    }

    /// Find the index of the first discriminant >= `byte`.
    /// Returns `len` if all discriminants are < `byte`.
    #[inline]
    pub fn find_child_lower_bound(&self, byte: u8) -> usize {
        crate::simd::hnode_find_child_lower_bound(self.ptr, self.len as usize, byte)
    }
}

/// Allocate a PairVec data buffer and write keys + values into it.
///
/// The allocation has `capacity` slots (only `keys.len()` of which
/// are valid). The values section is aligned to `align_of::<Trie>()`.
pub(crate) fn alloc_pairvec_data<const INLINE: usize, PREFIX: PrefixLen>(
    keys: &[u8],
    values: &[Trie<INLINE, PREFIX>],
    capacity: usize,
) -> *mut u8 {
    let len = keys.len();
    debug_assert_eq!(values.len(), len);
    debug_assert!(capacity >= len);

    let layout = PairVec::<INLINE, PREFIX>::layout(capacity);
    // SAFETY: layout has non-zero size (capacity >= len >= 1 for PairVec nodes).
    let ptr = unsafe { alloc::alloc(layout) };
    if ptr.is_null() {
        std::alloc::handle_alloc_error(layout);
    }

    // Zero the entire allocation first. This ensures all bytes are
    // initialized — critical for Miri safety since SIMD loads read
    // padding bytes in the values section (Trie union padding).
    unsafe { std::ptr::write_bytes(ptr, 0, layout.size()) };

    let values_off = PairVec::<INLINE, PREFIX>::values_offset(capacity);

    unsafe {
        // Write keys
        ptr.copy_from_nonoverlapping(keys.as_ptr(), len);
        // Write values
        let values_ptr = ptr.add(values_off) as *mut Trie<INLINE, PREFIX>;
        std::ptr::copy_nonoverlapping(values.as_ptr(), values_ptr, len);
    }

    ptr
}

/// Free a PairVec data buffer.
///
/// # Safety
/// `ptr` must point to a valid PairVec data allocation with the given
/// `capacity` (the `capacity` field from the PairVec struct, NOT `len`).
pub(crate) unsafe fn free_pairvec_data<const INLINE: usize, PREFIX: PrefixLen>(
    ptr: *mut u8,
    capacity: u8,
) {
    let layout = PairVec::<INLINE, PREFIX>::layout(capacity as usize);
    unsafe { alloc::dealloc(ptr, layout) };
}

/// Promote an INode (with `INLINE` children) to a PairVec (with
/// `INLINE + 1` children).
///
/// Takes the INode's discriminants, children pointer, and a new
/// (byte, child) to insert. Returns a PairVec with initial spare
/// capacity.
///
/// # Safety
/// `inode_children_ptr` must be a valid pointer to `inode_count` `Trie`
/// elements previously created via `Vec::into_boxed_slice()`.
pub(crate) fn promote_inode_to_pairvec<const INLINE: usize, PREFIX: PrefixLen>(
    inode_symbols: &[u8],
    inode_children_ptr: *const Trie<INLINE, PREFIX>,
    inode_count: usize,
    new_byte: u8,
    new_child: Trie<INLINE, PREFIX>,
    prefix_len: PREFIX,
) -> PairVec<INLINE, PREFIX> {
    let new_len = inode_count + 1;

    // Initial capacity: next power of 2 after new_len, capped at 255.
    // This gives ~50% spare capacity on average, with amortized growth.
    let initial_capacity = new_len.next_power_of_two().min(255);

    // Find insertion position for the new symbol.
    let insert_pos = inode_symbols[..inode_count]
        .iter()
        .position(|&s| s > new_byte)
        .unwrap_or(inode_count);

    // Build merged keys.
    let mut merged_keys = Vec::with_capacity(new_len);
    merged_keys.extend_from_slice(&inode_symbols[..insert_pos]);
    merged_keys.push(new_byte);
    merged_keys.extend_from_slice(&inode_symbols[insert_pos..]);

    // Build merged children.
    let old_children = unsafe { std::slice::from_raw_parts(inode_children_ptr, inode_count) };
    let mut merged_values = Vec::with_capacity(new_len);
    for i in 0..insert_pos {
        merged_values.push(unsafe { std::ptr::read(&old_children[i]) });
    }
    merged_values.push(new_child);
    for i in insert_pos..inode_count {
        merged_values.push(unsafe { std::ptr::read(&old_children[i]) });
    }

    let ptr =
        alloc_pairvec_data::<INLINE, PREFIX>(&merged_keys, &merged_values, initial_capacity);

    PairVec::new(new_len as u8, initial_capacity as u8, prefix_len, ptr)
}

/// Add a new (byte, child) pair to a PairVec.
///
/// If the PairVec has spare capacity, shifts elements in place — no
/// allocation needed. If full, doubles the capacity and reallocates.
///
/// Returns the updated PairVec. The caller is responsible for freeing
/// the old data buffer if it was replaced (only when reallocation
/// occurs, `free_pairvec_data` is called internally in that case).
pub(crate) fn add_child_to_pairvec<const INLINE: usize, PREFIX: PrefixLen>(
    mut pv: PairVec<INLINE, PREFIX>,
    byte: u8,
    new_child: Trie<INLINE, PREFIX>,
) -> PairVec<INLINE, PREFIX> {
    let old_len = pv.len as usize;
    let insert_pos = pv.find_child_lower_bound(byte);
    // The byte must not already exist in the sorted discriminants.
    debug_assert!(
        insert_pos == old_len || unsafe { *pv.ptr.add(insert_pos) != byte },
        "add_child_to_pairvec: byte {byte} already present"
    );

    if old_len < pv.capacity as usize {
        // ── Spare capacity: shift in place, no allocation ──
        unsafe {
            // Shift keys[insert_pos..old_len] right by 1 byte.
            let keys = pv.ptr;
            if insert_pos < old_len {
                std::ptr::copy(
                    keys.add(insert_pos),
                    keys.add(insert_pos + 1),
                    old_len - insert_pos,
                );
            }
            keys.add(insert_pos).write(byte);

            // Shift values[insert_pos..old_len] right by 1 position.
            let values_off = PairVec::<INLINE, PREFIX>::values_offset(pv.capacity as usize);
            let values = pv.ptr.add(values_off) as *mut Trie<INLINE, PREFIX>;
            if insert_pos < old_len {
                std::ptr::copy(
                    values.add(insert_pos),
                    values.add(insert_pos + 1),
                    old_len - insert_pos,
                );
            }
            values.add(insert_pos).write(new_child);
        }
        pv.len += 1;
        pv
    } else {
        // ── Full: double capacity and reallocate ──
        debug_assert!(
            (old_len as usize) < 255,
            "PairVec capacity overflow: cannot exceed 255 children"
        );
        let new_capacity = ((old_len as usize) * 2).min(255);

        // Build merged keys.
        let old_keys = unsafe { std::slice::from_raw_parts(pv.ptr, old_len) };
        let mut merged_keys = Vec::with_capacity(old_len + 1);
        merged_keys.extend_from_slice(&old_keys[..insert_pos]);
        merged_keys.push(byte);
        merged_keys.extend_from_slice(&old_keys[insert_pos..]);

        // Build merged values.
        let old_values = unsafe { pv.values() };
        let mut merged_values = Vec::with_capacity(old_len + 1);
        for i in 0..insert_pos {
            merged_values.push(unsafe { std::ptr::read(&old_values[i]) });
        }
        merged_values.push(new_child);
        for i in insert_pos..old_len {
            merged_values.push(unsafe { std::ptr::read(&old_values[i]) });
        }

        // Allocate new buffer.
        let new_ptr =
            alloc_pairvec_data::<INLINE, PREFIX>(&merged_keys, &merged_values, new_capacity);

        // Free old buffer.
        unsafe { free_pairvec_data::<INLINE, PREFIX>(pv.ptr, pv.capacity) };

        PairVec::new((old_len + 1) as u8, new_capacity as u8, pv.prefix_len, new_ptr)
    }
}