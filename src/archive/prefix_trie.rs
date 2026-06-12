use super::prefix_len::PrefixLen;
use super::TinyTrieMap;
use super::pairvec::{
    add_child_to_pairvec, free_pairvec_data, promote_inode_to_pairvec, PairVec,
};
use super::simd;


// Tag encoding:
//   0           Leaf (0 children)
//   1           Reserved (TAG_RESERVED, unused but reserved for future use)
//   2..=INLINE  INode with that many inline children
//   >INLINE     PairVec with that many entries (tag IS the length)

const TAG_LEAF: u8 = 0;
const TAG_RESERVED: u8 = 1; // Unused, reserved for future use.

//   ┌────────┬───────────────────────────────┐
//   │ PREFIX │ INLINE values with no padding │
//   ├────────┼───────────────────────────────┤
//   │ u8     │ 6, 14, 22, …                  │
//   ├────────┼───────────────────────────────┤
//   │ u16    │ 4, 12, 20, …                  │
//   ├────────┼───────────────────────────────┤
//   │ u32    │ 8, 16, 24, …                  │
//   └────────┴───────────────────────────────┘

/// Internal node: 2–INLINE children inline.
///
/// All bytes including `#[repr(C)]` padding are zero-initialized on
/// construction, ensuring Miri-safe SIMD loads.
///
/// Derives `Copy` for direct union field access. Ownership of the pointed-to
/// heap allocations is managed explicitly by `TinyTrie::drop` and
/// `free_subtree`.
#[repr(C)]
#[derive(Clone, Copy)]
struct INode<const INLINE: usize, PREFIX: PrefixLen> {
    tag: u8,
    prefix_len: PREFIX,
    symbols: [u8; INLINE],
    children: *mut Trie<INLINE, PREFIX>,
}

impl<const INLINE: usize, PREFIX: PrefixLen> INode<INLINE, PREFIX> {
    /// Create a fully-initialized INode with zeroed padding.
    fn new(tag: u8, prefix_len: PREFIX, symbols: [u8; INLINE], children: *mut Trie<INLINE, PREFIX>) -> Self {
        // Zero-initialize the entire struct to fill padding between
        // symbols and children, then overwrite with real values.
        let mut node: Self = unsafe { std::mem::zeroed() };
        node.tag = tag;
        node.prefix_len = prefix_len;
        node.symbols = symbols;
        node.children = children;
        node
    }
}

/// Heap-resident node: (INLINE+1)+ children.
/// Implemented as PairVec — see pairvec.rs for the struct definition.

/// Leaf node: stores a u64 index into the keys/values vecs.
///
/// Derives `Copy` for direct union field access.
#[repr(C)]
#[derive(Clone, Copy)]
struct Leaf {
    tag: u8,
    payload: [u8; 15],
}

/// The tagged union. Size determined by the largest variant (INode).
///
/// All variants are `Copy`, so `Trie` is `Copy` too. This enables direct union
/// field access (e.g., `unsafe { node.leaf.tag }`) without `ManuallyDrop`
/// wrappers, which reduces overhead in the lookup hot path.
///
/// Ownership of heap allocations pointed to by `INode::children` and
/// `PairVec::ptr` is managed explicitly — see the module-level safety comment.
#[repr(C)]
pub(crate) union Trie<const INLINE: usize, PREFIX: PrefixLen> {
    inode: INode<INLINE, PREFIX>,
    pairvec: PairVec<INLINE, PREFIX>,
    leaf: Leaf,
}

// --- Tag access ---

impl<const INLINE: usize, PREFIX: PrefixLen> Trie<INLINE, PREFIX> {
    #[inline(always)]
    fn tag(&self) -> u8 {
        // SAFETY: The tag byte is at offset 0 in all union variants (Leaf,
        // INode, PairVec) due to #[repr(C)] layout. Reading byte 0 of the
        // union gives the tag regardless of the active variant.
        unsafe { self.leaf.tag }
    }

    fn as_leaf(&self) -> Option<&Leaf> {
        if self.tag() == TAG_LEAF {
            // SAFETY: tag confirms the active variant is Leaf.
            Some(unsafe { &*(&raw const self.leaf) })
        } else {
            None
        }
    }

    /// Number of children for an internal node (INode or PairVec).
    /// For internal nodes, `tag as usize` IS the child count.
    /// Must not be called on a Leaf (tag 0) or Reserved (tag 1).
    fn child_count(&self) -> usize {
        let tag = self.tag();
        debug_assert!(tag >= 2, "child_count called on non-internal node (tag={tag})");
        tag as usize
    }

    // --- Inline hot-path methods ---
    //
    // These dispatch on tag internally and are #[inline(always)] so the
    // compiler can inline them into callers, avoiding the overhead of
    // constructing an intermediate enum.

    /// Returns the prefix length for an internal node.
    /// Must not be called on a Leaf.
    #[inline(always)]
    fn prefix_len(&self) -> usize {
        let tag = self.tag();
        debug_assert!(tag >= 2);
        if tag as usize <= INLINE {
            unsafe { self.inode }.prefix_len.into_usize()
        } else {
            unsafe { self.pairvec }.prefix_len.into_usize()
        }
    }

    /// Find a child by its discriminant byte.
    /// Returns the child index, or None if not found.
    #[inline(always)]
    fn find_child(&self, byte: u8) -> Option<usize> {
        let tag = self.tag();
        debug_assert!(tag >= 2);
        if tag as usize <= INLINE {
            unsafe { self.inode }.find_child(byte)
        } else {
            unsafe { self.pairvec }.find_child(byte)
        }
    }

    /// Find first child with discriminant >= byte.
    #[inline(always)]
    fn find_child_lower_bound(&self, byte: u8) -> usize {
        let tag = self.tag();
        debug_assert!(tag >= 2);
        if tag as usize <= INLINE {
            unsafe { self.inode }.find_child_lower_bound(byte)
        } else {
            unsafe { self.pairvec }.find_child_lower_bound(byte)
        }
    }

    /// Returns the children slice for an internal node.
    #[inline(always)]
    fn children(&self) -> &[Trie<INLINE, PREFIX>] {
        let tag = self.tag();
        debug_assert!(tag >= 2);
        if tag as usize <= INLINE {
            let inode = unsafe { &*(&raw const self.inode) };
            unsafe { std::slice::from_raw_parts(inode.children, tag as usize) }
        } else {
            unsafe { (&*(&raw const self.pairvec)).values() }
        }
    }

    /// Returns the discriminant bytes (symbols/keys) for an internal node.
    #[inline(always)]
    fn symbols(&self) -> &[u8] {
        let tag = self.tag();
        debug_assert!(tag >= 2);
        if tag as usize <= INLINE {
            &unsafe { &*(&raw const self.inode) }.symbols[..tag as usize]
        } else {
            unsafe { (&*(&raw const self.pairvec)).keys() }
        }
    }
}

// --- InternalNodeOwned: unified owned/mutable view over INode and PairVec ---

enum InternalNodeOwned<const INLINE: usize, PREFIX: PrefixLen> {
    Inline(INode<INLINE, PREFIX>),
    PairVec(PairVec<INLINE, PREFIX>),
}

impl<const INLINE: usize, PREFIX: PrefixLen> InternalNodeOwned<INLINE, PREFIX> {
    /// Deconstruct a `Box<Trie>` into an owned internal node.
    ///
    /// Uses `ptr::read` to extract the variant data (an ownership transfer),
    /// then drops the Box (freeing only the 16/24-byte node allocation —
    /// not the children/data pointers carried in the variant).
    fn from_box(boxed: Box<Trie<INLINE, PREFIX>>) -> Self {
        let tag = boxed.tag();
        match tag {
            t if t >= 2 && t as usize <= INLINE => {
                // SAFETY: tag confirms the active variant is INode.
                // Trie is Copy, so reading the inode field is a bitwise copy.
                let inode = unsafe { (*boxed).inode };
                Self::Inline(inode)
            }
            _ if tag > INLINE as u8 => {
                // SAFETY: tag confirms the active variant is PairVec.
                let pv = unsafe { (*boxed).pairvec };
                Self::PairVec(pv)
            }
            _ => panic!("from_box called on Leaf or Reserved node"),
        }
    }

    fn prefix_len(&self) -> usize {
        match self {
            Self::Inline(inode) => inode.prefix_len.into_usize(),
            Self::PairVec(pv) => pv.prefix_len.into_usize(),
        }
    }

    fn find_child(&self, byte: u8) -> Option<usize> {
        match self {
            Self::Inline(inode) => inode.find_child(byte),
            Self::PairVec(pv) => pv.find_child(byte),
        }
    }

    fn find_child_lower_bound(&self, byte: u8) -> usize {
        match self {
            Self::Inline(inode) => inode.find_child_lower_bound(byte),
            Self::PairVec(pv) => pv.find_child_lower_bound(byte),
        }
    }

    fn child_count(&self) -> usize {
        match self {
            Self::Inline(inode) => inode.tag as usize,
            Self::PairVec(pv) => pv.len as usize,
        }
    }

    /// Descend to the leftmost leaf and return its key.
    fn first_key<'a>(&self, keys: &'a [Vec<u8>]) -> &'a [u8] {
        let first: &Trie<INLINE, PREFIX> = match self {
            Self::Inline(inode) => unsafe { &*inode.children },
            Self::PairVec(pv) => {
                let values_off = PairVec::<INLINE, PREFIX>::values_offset(pv.capacity as usize);
                unsafe { &*(pv.ptr.add(values_off) as *const Trie<INLINE, PREFIX>) }
            }
        };
        let mut node = first;
        loop {
            match node.tag() {
                TAG_LEAF => {
                    let index = unsafe { node.leaf.index() } as usize;
                    return &keys[index];
                }
                _ => {
                    node = &node.children()[0];
                }
            }
        }
    }

    fn into_trie(self) -> Trie<INLINE, PREFIX> {
        match self {
            Self::Inline(inode) => Trie { inode },
            Self::PairVec(pv) => Trie { pairvec: pv },
        }
    }

    /// Read a child by index (ownership transfer via ptr::read).
    /// The caller takes ownership of the returned value.
    fn read_child(&self, idx: usize) -> Trie<INLINE, PREFIX> {
        match self {
            Self::Inline(inode) => {
                unsafe { std::ptr::read(inode.children.add(idx)) }
            }
            Self::PairVec(pv) => {
                let values_off = PairVec::<INLINE, PREFIX>::values_offset(pv.capacity as usize);
                let child_ptr = unsafe { pv.ptr.add(values_off) as *const Trie<INLINE, PREFIX> };
                unsafe { std::ptr::read(child_ptr.add(idx)) }
            }
        }
    }

    /// Replace a child in-place and return the updated Trie.
    ///
    /// For INode, this writes the new child directly into the existing
    /// children array — O(1), no allocation. For PairVec, same in-place
    /// write into the values section.
    fn replace_child(self, idx: usize, new_child: Trie<INLINE, PREFIX>) -> Trie<INLINE, PREFIX> {
        match self {
            Self::Inline(inode) => {
                unsafe { std::ptr::write(inode.children.add(idx), new_child) };
                Trie { inode }
            }
            Self::PairVec(pv) => {
                let values_off = PairVec::<INLINE, PREFIX>::values_offset(pv.capacity as usize);
                let child_ptr = unsafe { pv.ptr.add(values_off) as *mut Trie<INLINE, PREFIX> };
                unsafe { std::ptr::write(child_ptr.add(idx), new_child) };
                Trie { pairvec: pv }
            }
        }
    }

    /// Add a new (byte, child) pair and return the updated Trie.
    fn add_child(self, byte: u8, child: Trie<INLINE, PREFIX>) -> Trie<INLINE, PREFIX> {
        match self {
            Self::Inline(inode) => add_child_to_inode(inode, byte, child),
            Self::PairVec(pv) => Trie { pairvec: add_child_to_pairvec(pv, byte, child) },
        }
    }

    /// Split this node's prefix at the point where the new key diverges.
    ///
    /// Creates a new parent node with two children: this node (with a
    /// shortened prefix) and a new leaf for the inserted key.
    fn split_prefix(
        self,
        diverged_prefix_len: u8,
        existing_byte: u8,
        new_byte: u8,
        new_index: usize,
    ) -> Trie<INLINE, PREFIX> {
        let prefix_len = self.prefix_len();
        let remaining_prefix = (prefix_len - diverged_prefix_len as usize - 1) as u8;

        let existing_child = match self {
            Self::Inline(inode) => {
                let child_inode = INode::new(
                    inode.tag,
                    PREFIX::from(remaining_prefix),
                    inode.symbols,
                    inode.children,
                );
                Trie { inode: child_inode }
            }
            Self::PairVec(pv) => {
                let child_pv = PairVec::new(
                    pv.len,
                    pv.capacity,
                    PREFIX::from(remaining_prefix),
                    pv.ptr,
                );
                Trie { pairvec: child_pv }
            }
        };

        let new_child = Trie { leaf: Leaf::new(new_index as u64) };

        let OrderedPair { sym_lo, child_lo, sym_hi, child_hi } = order_pair(
            existing_byte, existing_child, new_byte, new_child,
        );

        make_inode_2(diverged_prefix_len, sym_lo, sym_hi, child_lo, child_hi)
    }
}

// --- Prefix divergence ---

/// Result of comparing a new key against an existing key from a given offset.
enum PrefixResult {
    /// Keys diverge within the checked range.
    /// `prefix_len` is the number of matching bytes before the divergence.
    /// `existing_byte` and `new_byte` are the differing bytes at the divergence
    /// point. If `new_byte` is 0, the new key ended before the existing key.
    Diverged {
        prefix_len: u8,
        existing_byte: u8,
        new_byte: u8,
    },
    /// Keys match through the entire checked prefix of length `max_prefix_len`.
    /// `byte_offset` is `offset + max_prefix_len` (position of the discriminating
    /// byte after the shared prefix).
    Matched {
        byte_offset: usize,
    },
}

/// Compare `new_key` against `existing_key` starting at `offset`, checking
/// up to `max_prefix_len` bytes. If they diverge within that range, returns
/// `PrefixResult::Diverged`. If they match through the entire prefix, returns
/// `PrefixResult::Matched`.
///
/// For Leaf nodes, pass `max_prefix_len = existing_key.len() - offset`.
/// For internal nodes, pass `max_prefix_len = node.prefix_len()`.
fn find_prefix_divergence(
    existing_key: &[u8],
    new_key: &[u8],
    offset: usize,
    max_prefix_len: usize,
) -> PrefixResult {
    for i in 0..max_prefix_len {
        let ki = offset + i;
        if ki >= new_key.len() {
            return PrefixResult::Diverged {
                prefix_len: i as u8,
                // NOTE: When the new key is shorter than the existing key, existing_byte
                // falls back to the actual key byte (which includes the null terminator).
                // This is safe because: (1) user keys can't contain 0x00, so 0x00 uniquely
                // identifies the end-of-key marker; (2) the null terminator is always present
                // in stored keys. The sentinel value of 0 for "new key ended" is distinct
                // from any user key byte because of the no-null-byte invariant.
                existing_byte: if ki < existing_key.len() { existing_key[ki] } else { 0 },
                new_byte: 0,
            };
        }
        if ki >= existing_key.len() || new_key[ki] != existing_key[ki] {
            return PrefixResult::Diverged {
                prefix_len: i as u8,
                existing_byte: if ki < existing_key.len() { existing_key[ki] } else { 0 },
                new_byte: new_key[ki],
            };
        }
    }
    PrefixResult::Matched {
        byte_offset: offset + max_prefix_len,
    }
}

// --- Leaf helpers ---

impl Leaf {
    fn index(&self) -> u64 {
        u64::from_le_bytes(self.payload[0..8].try_into().unwrap())
    }

    fn new(index: u64) -> Self {
        let mut leaf = Leaf { tag: TAG_LEAF, payload: [0u8; 15] };
        leaf.payload[0..8].copy_from_slice(&index.to_le_bytes());
        leaf
    }
}

// --- INode child lookup (SIMD via portable_simd) ---

impl<const INLINE: usize, PREFIX: PrefixLen> INode<INLINE, PREFIX> {
    fn find_child(&self, byte: u8) -> Option<usize> {
        let symbols_offset = core::mem::offset_of!(Self, symbols);
        simd::inode_find_child(
            self as *const Self as *const u8,
            symbols_offset,
            self.tag as usize,
            byte,
        )
    }

    /// Returns the index of the first symbol >= `byte`,
    /// or `tag` (past end) if all symbols are < `byte`.
    fn find_child_lower_bound(&self, byte: u8) -> usize {
        let symbols_offset = core::mem::offset_of!(Self, symbols);
        simd::inode_find_child_lower_bound(
            self as *const Self as *const u8,
            symbols_offset,
            self.tag as usize,
            byte,
        )
    }
}

// --- PairVec access helpers (defined in pairvec.rs) ---

/// Align `val` up to the next multiple of `align` (must be a power of 2).
pub(crate) const fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// Free a children slice previously allocated via `Vec::into_boxed_slice()`.
///
/// Reconstructs the fat pointer from the thin `*mut Trie` and count,
/// then passes it to `Box::from_raw` for deallocation.
///
/// # Safety
/// `ptr` must point to a valid allocation of `count` `Trie` elements
/// previously created via `Vec::into_boxed_slice()`.
pub(crate) unsafe fn free_children_slice<const INLINE: usize, PREFIX: PrefixLen>(
    ptr: *mut Trie<INLINE, PREFIX>,
    count: usize,
) {
    let fat = std::ptr::slice_from_raw_parts_mut(ptr, count);
    unsafe { drop(Box::from_raw(fat)) };
}

// --- Iterator helpers ---

/// Descend from `node` to the leftmost (smallest) leaf, pushing
/// `(parent, 0)` entries onto `stack` along the way.
/// Returns the leaf index.
fn leftmost_leaf<const INLINE: usize, PREFIX: PrefixLen>(
    node: *const Trie<INLINE, PREFIX>,
    stack: &mut Vec<(*const Trie<INLINE, PREFIX>, usize)>,
) -> usize {
    let mut node = node;
    loop {
        let node_ref = unsafe { &*node };
        match node_ref.tag() {
            TAG_LEAF => {
                return unsafe { node_ref.leaf.index() } as usize;
            }
            _ => {
                stack.push((node, 0));
                node = std::ptr::from_ref(&node_ref.children()[0]);
            }
        }
    }
}

/// Descend from `node` to the rightmost (largest) leaf, pushing
/// `(parent, child_count - 1)` entries onto `stack` along the way.
/// Returns the leaf index.
fn rightmost_leaf<const INLINE: usize, PREFIX: PrefixLen>(
    node: *const Trie<INLINE, PREFIX>,
    stack: &mut Vec<(*const Trie<INLINE, PREFIX>, usize)>,
) -> usize {
    let mut node = node;
    loop {
        let node_ref = unsafe { &*node };
        match node_ref.tag() {
            TAG_LEAF => {
                return unsafe { node_ref.leaf.index() } as usize;
            }
            _ => {
                let last = node_ref.child_count() - 1;
                stack.push((node, last));
                node = std::ptr::from_ref(&node_ref.children()[last]);
            }
        }
    }
}

// --- TinyTrie ---

pub struct TinyTrie<T: Clone, const INLINE: usize, PREFIX: PrefixLen> {
    root: Option<Box<Trie<INLINE, PREFIX>>>,
    keys: Vec<Vec<u8>>,
    values: Vec<T>,
}

// Safety: the raw pointers inside Trie/INode/PairVec are only dereferenced in
// &self methods (get, iter — read-only) and &mut self methods (insert, Drop).
// Sharing &TinyTrie across threads is safe because no &self method mutates
// through the pointers.
unsafe impl<T: Clone + Sync, const INLINE: usize, PREFIX: PrefixLen + Sync> Sync
    for TinyTrie<T, INLINE, PREFIX>
{
}

impl<T: Clone, const INLINE: usize, PREFIX: PrefixLen> TinyTrie<T, INLINE, PREFIX> {
    pub fn new() -> Self {
        TinyTrie { root: None, keys: Vec::new(), values: Vec::new() }
    }

    /// Look up a null-terminated key and return its index, or `None` if not found.
    ///
    /// The key **must** end with a null byte (`0x00`). Use [`null_terminate`]
    /// to add one if needed. No allocation is performed on the lookup path.
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        debug_assert!(key.last() == Some(&0), "key must be null-terminated");
        let root = self.root.as_ref()?;
        let mut node: &Trie<INLINE, PREFIX> = root;
        let mut offset = 0usize;

        loop {
            let tag = node.tag();
            match tag {
                TAG_LEAF => {
                    let leaf = unsafe { node.leaf };
                    let index = leaf.index() as usize;
                    if index < self.keys.len() && self.keys[index] == key {
                        return Some(index);
                    }
                    return None;
                }
                t if t as usize <= INLINE => {
                    // SAFETY: tag confirms INode variant is active.
                    let inode = unsafe { node.inode };
                    offset += inode.prefix_len.into_usize();
                    if offset >= key.len() { return None; }
                    let byte = key[offset];
                    offset += 1;
                    let idx = inode.find_child(byte)?;
                    let children = unsafe { std::slice::from_raw_parts(inode.children, t as usize) };
                    node = &children[idx];
                }
                _ => {
                    // SAFETY: tag confirms PairVec variant is active.
                    let pv = unsafe { node.pairvec };
                    offset += pv.prefix_len.into_usize();
                    if offset >= key.len() { return None; }
                    let byte = key[offset];
                    offset += 1;
                    let idx = pv.find_child(byte)?;
                    let values_off = PairVec::<INLINE, PREFIX>::values_offset(pv.capacity as usize);
                    let children_ptr = unsafe { pv.ptr.add(values_off) as *const Trie<INLINE, PREFIX> };
                    let children = unsafe { std::slice::from_raw_parts(children_ptr, pv.len as usize) };
                    node = &children[idx];
                }
            }
        }
    }

    /// Insert a new key-value pair. Returns `Ok(index)` on success.
    /// Returns `Err(())` if the key already exists.
    /// Panics if the key contains a null byte (0x00).
    ///
    /// The key must **not** contain null bytes. Internally, a `0x00` terminator
    /// is appended before storing.
    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        assert!(!key.contains(&0), "key must not contain null bytes");
        let mut nt_key = key;
        nt_key.push(0);

        let index = self.keys.len();
        self.keys.push(nt_key.clone());
        self.values.push(value);

        match &self.root {
            None => {
                let leaf = Leaf::new(index as u64);
                self.root = Some(Box::new(Trie { leaf }));
                Ok(index)
            }
            Some(_) => {
                match self.insert_into_root(index, &nt_key) {
                    Ok(root) => {
                        self.root = Some(root);
                        Ok(index)
                    }
                    Err(()) => {
                        self.keys.pop();
                        self.values.pop();
                        Err(())
                    }
                }
            }
        }
    }

    fn insert_into_root(&mut self, new_index: usize, new_key: &[u8]) -> Result<Box<Trie<INLINE, PREFIX>>, ()> {
        let old_root = self.root.take().unwrap();
        self.insert_into_node(old_root, new_key, new_index, 0)
    }

    fn insert_into_node(
        &mut self,
        node: Box<Trie<INLINE, PREFIX>>,
        new_key: &[u8],
        new_index: usize,
        offset: usize,
    ) -> Result<Box<Trie<INLINE, PREFIX>>, ()> {
        match node.tag() {
            TAG_LEAF => {
                // SAFETY: tag confirms the active variant is Leaf.
                // Trie is Copy, so reading the leaf field is a bitwise copy.
                let leaf = unsafe { (*node).leaf };
                let existing_key = &self.keys[leaf.index() as usize];

                match find_prefix_divergence(existing_key, new_key, offset, existing_key.len() - offset) {
                    PrefixResult::Diverged { prefix_len, existing_byte, new_byte } => {
                        let existing_child = Trie { leaf: Leaf::new(leaf.index()) };
                        let new_child = Trie { leaf: Leaf::new(new_index as u64) };
                        let OrderedPair { sym_lo, child_lo, sym_hi, child_hi } = order_pair(
                            existing_byte, existing_child, new_byte, new_child,
                        );
                        Ok(Box::new(make_inode_2(prefix_len, sym_lo, sym_hi, child_lo, child_hi)))
                    }
                    PrefixResult::Matched { .. } => Err(()),
                }
            }

            _ => {
                let internal = InternalNodeOwned::from_box(node);
                let prefix_len = internal.prefix_len();
                let existing_key = internal.first_key(&self.keys);

                match find_prefix_divergence(existing_key, new_key, offset, prefix_len) {
                    PrefixResult::Diverged { prefix_len: matched, existing_byte, new_byte } => {
                        Ok(Box::new(internal.split_prefix(matched, existing_byte, new_byte, new_index)))
                    }
                    PrefixResult::Matched { byte_offset } => {
                        let byte = new_key[byte_offset];
                        match internal.find_child(byte) {
                            Some(child_idx) => {
                                // Descend: read old child, recurse, write back in-place.
                                let old_child = internal.read_child(child_idx);
                                let new_child_box = self.insert_into_node(
                                    Box::new(old_child), new_key, new_index, byte_offset + 1,
                                )?;
                                Ok(Box::new(internal.replace_child(child_idx, *new_child_box)))
                            }
                            None => {
                                let new_leaf = Trie { leaf: Leaf::new(new_index as u64) };
                                Ok(Box::new(internal.add_child(byte, new_leaf)))
                            }
                        }
                    }
                }
            }
        }
    }

    /// Returns the number of key-value pairs stored in the trie.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns `true` if the trie contains no key-value pairs.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Look up a null-terminated key and return a reference to its value,
    /// or `None` if not found.
    ///
    /// The key **must** end with a null byte (`0x00`), matching the
    /// [`get`](Self::get) contract. This is a convenience wrapper around
    /// [`get`](Self::get) that returns `&T` directly instead of the index.
    pub fn get_value(&self, key: &[u8]) -> Option<&T> {
        let idx = self.get(key)?;
        Some(&self.values[idx])
    }

    /// Consume the trie and return the stored keys and values.
    ///
    /// Frees all heap-allocated trie nodes before extracting the key/value
    /// vectors, so the returned data owns its memory with no dangling pointers.
    pub fn into_keys_values(mut self) -> (Vec<Vec<u8>>, Vec<T>) {
        // Free all heap-allocated trie nodes first.
        if let Some(ref root) = self.root {
            unsafe { Self::free_subtree(root) };
        }
        // Setting root to None makes Drop a no-op (it guards on `if let Some`).
        self.root = None;
        // Now safe to take ownership of the fields — Drop runs on empty Vecs.
        let keys = std::mem::take(&mut self.keys);
        let values = std::mem::take(&mut self.values);
        (keys, values)
    }

    /// Return a bidirectional iterator positioned at the first (leftmost) key.
    pub fn iter(&self) -> TrieIter<'_, T, INLINE, PREFIX> {
        let mut stack = Vec::new();
        let current = if let Some(ref root) = self.root {
            Some(leftmost_leaf(std::ptr::from_ref(&**root), &mut stack))
        } else {
            None
        };
        TrieIter { trie: self, stack, current }
    }

    /// Return a bidirectional iterator positioned at the last (rightmost) key.
    /// Returns an exhausted iterator if the trie is empty.
    pub fn iter_last(&self) -> TrieIter<'_, T, INLINE, PREFIX> {
        let mut stack = Vec::new();
        let current = if let Some(ref root) = self.root {
            Some(rightmost_leaf(std::ptr::from_ref(&**root), &mut stack))
        } else {
            None
        };
        TrieIter { trie: self, stack, current }
    }
}

impl<T: Clone, const INLINE: usize, PREFIX: PrefixLen> Default for TinyTrie<T, INLINE, PREFIX> {
    fn default() -> Self { Self::new() }
}

impl<T: Clone, const INLINE: usize, PREFIX: PrefixLen> Drop for TinyTrie<T, INLINE, PREFIX> {
    fn drop(&mut self) {
        if let Some(ref root) = self.root {
            unsafe { Self::free_subtree(root) };
        }
        // self.root (Option<Box<Trie>>) drops automatically after this,
        // freeing the root node's 16/24 bytes. Trie is not Copy and has
        // no Drop impl, so the Box<Trie> does not follow the (now-dangling)
        // children/data pointers — it only frees its own allocation.
    }
}

impl<T: Clone, const INLINE: usize, PREFIX: PrefixLen> TinyTrie<T, INLINE, PREFIX> {
    /// Recursively free all heap allocations reachable from `node`,
    /// excluding the node's own allocation (the caller owns that).
    ///
    /// # Safety
    /// Caller must ensure `node` is a valid, properly-initialized trie node
    /// and that no other references to any sub-allocations exist.
    unsafe fn free_subtree(node: &Trie<INLINE, PREFIX>) {
        match node.tag() {
            TAG_LEAF => {}
            t if t as usize <= INLINE => {
                let inode = unsafe { &*(&raw const node.inode) };
                for child in node.children() {
                    unsafe { Self::free_subtree(child) };
                }
                unsafe { free_children_slice(inode.children, inode.tag as usize) };
            }
            _ => {
                let pv = unsafe { &*(&raw const node.pairvec) };
                for child in node.children() {
                    unsafe { Self::free_subtree(child) };
                }
                unsafe { free_pairvec_data::<INLINE, PREFIX>(pv) };
            }
        }
    }
}

// --- TrieIter ---

/// Bidirectional cursor iterator over a `TinyTrie`.
///
/// Created by `TinyTrie::iter()`. Positioned at the first key initially.
/// Use `current()` to read without advancing, `next()`/`prev()` to move.
pub struct TrieIter<'a, T: Clone, const INLINE: usize, PREFIX: PrefixLen> {
    trie: &'a TinyTrie<T, INLINE, PREFIX>,
    /// Stack of (parent_node, child_index_descended_into).
    /// The current leaf is the child at the bottom of the stack.
    stack: Vec<(*const Trie<INLINE, PREFIX>, usize)>,
    /// Index of the current leaf in `trie.keys`/`trie.values`, or None if exhausted.
    current: Option<usize>,
}

impl<T: Clone, const INLINE: usize, PREFIX: PrefixLen> TrieIter<'_, T, INLINE, PREFIX> {
    /// Return the current key and value without advancing.
    ///
    /// The key is returned without the null terminator (matching the `insert` API).
    /// Returns `None` if the iterator is exhausted.
    pub fn current(&self) -> Option<(&[u8], &T)> {
        self.current.map(|idx| {
            let key = &self.trie.keys[idx];
            // Strip the null terminator for presentation
            let key = &key[..key.len().saturating_sub(1)];
            (key, &self.trie.values[idx])
        })
    }

    /// Advance to the next key in sorted order and return it.
    /// Returns `None` if already at the last key — the cursor stays at
    /// the current position and `prev()` can still go backward.
    pub fn next(&mut self) -> Option<(&[u8], &T)> {
        // Walk backwards through the stack (without popping) to find a node
        // with a next sibling. This preserves the stack when exhausted,
        // allowing prev() to work after next() returns None.
        for i in (0..self.stack.len()).rev() {
            let (node, child_idx) = self.stack[i];
            let node_ref = unsafe { &*node };
            let count = node_ref.child_count();
            if child_idx + 1 < count {
                let new_idx = child_idx + 1;
                // Truncate stack to this level, update the child index
                self.stack.truncate(i + 1);
                self.stack[i].1 = new_idx;
                // Descend to leftmost leaf from the new sibling
                let children = node_ref.children();
                let leaf_idx = leftmost_leaf(
                    std::ptr::from_ref(&children[new_idx]),
                    &mut self.stack,
                );
                self.current = Some(leaf_idx);
                return self.current();
            }
        }
        // No next key found — cursor stays at current position
        None
    }

    /// Move to the previous key in sorted order and return it.
    /// Returns `None` if already at the first key — the cursor stays at
    /// the current position and `next()` can still go forward.
    pub fn prev(&mut self) -> Option<(&[u8], &T)> {
        for i in (0..self.stack.len()).rev() {
            let (node, child_idx) = self.stack[i];
            if child_idx > 0 {
                let new_idx = child_idx - 1;
                self.stack.truncate(i + 1);
                self.stack[i].1 = new_idx;
                let node_ref = unsafe { &*node };
                let children = node_ref.children();
                let leaf_idx = rightmost_leaf(
                    std::ptr::from_ref(&children[new_idx]),
                    &mut self.stack,
                );
                self.current = Some(leaf_idx);
                return self.current();
            }
        }
        // No previous key — cursor stays at current position
        None
    }

    /// Position at the first key >= `key` (or where `key` would go).
    ///
    /// The key **must** end with a null byte (`0x00`). Use [`null_terminate`]
    /// to add one if needed. If no such key exists, the iterator becomes exhausted.
    pub fn seek(&mut self, key: &[u8]) {
        debug_assert!(key.last() == Some(&0), "key must be null-terminated");
        let nt_key = key;  // key is already null-terminated
        self.stack.clear();
        self.current = None;

        let Some(root) = &self.trie.root else { return };
        let mut node: *const Trie<INLINE, PREFIX> = &**root;
        let mut offset = 0usize;

        loop {
            let node_ref = unsafe { &*node };

            match node_ref.tag() {
                TAG_LEAF => {
                    let leaf_idx = unsafe { node_ref.leaf.index() } as usize;
                    self.current = Some(leaf_idx);
                    // If leaf key < seek key, advance forward
                    if self.trie.keys[leaf_idx].as_slice() < key {
                        self.next();
                    }
                    return;
                }

                _ => {
                    offset += node_ref.prefix_len();

                    if offset >= nt_key.len() {
                        // Seek key exhausted — leftmost leaf is >= seek key
                        self.stack.push((node, 0));
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&node_ref.children()[0]), &mut self.stack,
                        ));
                        return;
                    }

                    let byte = nt_key[offset];
                    offset += 1;
                    let lb = node_ref.find_child_lower_bound(byte);
                    let child_count = node_ref.child_count();

                    if lb < child_count && node_ref.symbols()[lb] == byte {
                        // Exact match — descend
                        self.stack.push((node, lb));
                        node = std::ptr::from_ref(&node_ref.children()[lb]);
                    } else if lb < child_count {
                        // First child > byte — its leftmost leaf is >= seek key
                        self.stack.push((node, lb));
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&node_ref.children()[lb]), &mut self.stack,
                        ));
                        return;
                    } else {
                        // All children < byte — push sentinel, advance via next()
                        self.stack.push((node, child_count));
                        self.next();
                        return;
                    }
                }
            }
        }
    }
}

// --- Free functions ---

struct OrderedPair<const INLINE: usize, PREFIX: PrefixLen> {
    sym_lo: u8,
    child_lo: Trie<INLINE, PREFIX>,
    sym_hi: u8,
    child_hi: Trie<INLINE, PREFIX>,
}

fn order_pair<const INLINE: usize, PREFIX: PrefixLen>(
    byte_a: u8, child_a: Trie<INLINE, PREFIX>,
    byte_b: u8, child_b: Trie<INLINE, PREFIX>,
) -> OrderedPair<INLINE, PREFIX> {
    if byte_a <= byte_b {
        OrderedPair { sym_lo: byte_a, child_lo: child_a, sym_hi: byte_b, child_hi: child_b }
    } else {
        OrderedPair { sym_lo: byte_b, child_lo: child_b, sym_hi: byte_a, child_hi: child_a }
    }
}

/// Append a null byte (`0x00`) to a key, producing a null-terminated key
/// suitable for [`TinyTrie::get`] and [`TrieIter::seek`].
///
/// ```
/// let nt = tiny_trie::null_terminate(b"hello");
/// assert_eq!(&nt[..], b"hello\0");
/// ```
pub fn null_terminate(key: &[u8]) -> Vec<u8> {
    let mut v = key.to_vec();
    v.push(0);
    v
}

fn make_inode_2<const INLINE: usize, PREFIX: PrefixLen>(
    prefix_len: u8,
    sym_lo: u8,
    sym_hi: u8,
    child_lo: Trie<INLINE, PREFIX>,
    child_hi: Trie<INLINE, PREFIX>,
) -> Trie<INLINE, PREFIX> {
    let mut symbols = [0u8; INLINE];
    symbols[0] = sym_lo;
    symbols[1] = sym_hi;
    let children = vec![child_lo, child_hi];
    let children_ptr = Box::into_raw(children.into_boxed_slice()) as *mut Trie<INLINE, PREFIX>;
    Trie {
        inode: INode::new(2, PREFIX::from(prefix_len), symbols, children_ptr),
    }
}

fn add_child_to_inode<const INLINE: usize, PREFIX: PrefixLen>(
    inode: INode<INLINE, PREFIX>,
    byte: u8,
    new_child: Trie<INLINE, PREFIX>,
) -> Trie<INLINE, PREFIX> {
    let old_tag = inode.tag as usize;

    if old_tag < INLINE {
        // Room in the inline symbols array — add the child.
        let new_tag = (old_tag + 1) as u8;
        let insert_pos = inode.symbols[..old_tag]
            .iter().position(|&s| s > byte).unwrap_or(old_tag);

        let mut symbols = inode.symbols;
        symbols.copy_within(insert_pos..old_tag, insert_pos + 1);
        symbols[insert_pos] = byte;

        let old_children = unsafe { std::slice::from_raw_parts(inode.children, old_tag) };
        let mut new_children = Vec::with_capacity(new_tag as usize);
        for i in 0..insert_pos {
            new_children.push(unsafe { std::ptr::read(&old_children[i]) });
        }
        new_children.push(new_child);
        for i in insert_pos..old_tag {
            new_children.push(unsafe { std::ptr::read(&old_children[i]) });
        }
        let new_children_ptr = Box::into_raw(new_children.into_boxed_slice())
            as *mut Trie<INLINE, PREFIX>;

        unsafe { free_children_slice(inode.children, old_tag); }

        Trie {
            inode: INode::new(new_tag, inode.prefix_len, symbols, new_children_ptr),
        }
    } else {
        // INode is full — promote to PairVec.
        let new_pv = promote_inode_to_pairvec(
            &inode.symbols[..old_tag],
            inode.children,
            old_tag,
            byte,
            new_child,
            inode.prefix_len,
        );
        unsafe { free_children_slice(inode.children, old_tag); }
        Trie { pairvec: new_pv }
    }
}

impl TinyTrieMap for TinyTrie<usize, 6, u8> {
    fn trie_new() -> Self { Self::new() }
    fn trie_insert(&mut self, key: Vec<u8>, value: usize) { self.insert(key, value).unwrap(); }
    fn trie_get(&self, key: &[u8]) -> Option<usize> { self.get(key) }
    fn trie_iter_fwd(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.next() { f(k, v); }
    }
    fn trie_iter_rev(&self, mut f: impl FnMut(&[u8], &usize)) {
        let mut it = self.iter_last();
        if let Some((k, v)) = it.current() { f(k, v); }
        while let Some((k, v)) = it.prev() { f(k, v); }
    }
    fn trie_len(&self) -> usize { self.len() }
    // trie_optimize: default no-op (TinyTrie has no optimize)
}

#[cfg(test)]
#[path = "tests/prefix_trie.rs"]
mod tests;
