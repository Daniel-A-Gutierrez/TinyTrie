//! Compact DFA String Index
//!
//! A prefix-compressed radix trie with existence guarantee, viewed as a
//! deterministic finite automaton. Node size is determined by const generics:
//! - `INLINE`: max inline children count (tag 2..=INLINE)
//! - `PREFIX`: prefix length type (u8, u16, or u32)

#![feature(portable_simd)]

mod prefix_len;
use prefix_len::PrefixLen;

mod simd;

// Tag encoding:
//   0           HNode (heap-allocated children)
//   1           Leaf
//   2..=INLINE  INode with that many inline children

const TAG_HNODE: u8 = 0;
const TAG_LEAF: u8 = 1;

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
#[repr(C)]
#[derive(Clone, Copy)]
struct INode<const INLINE: usize, PREFIX: PrefixLen> {
    tag: u8,
    prefix_len: PREFIX,
    symbols: [u8; INLINE],
    children: *mut Trie<INLINE, PREFIX>,
}

/// Heap node: (INLINE+1)+ children.
#[repr(C)]
#[derive(Clone, Copy)]
struct HNode<PREFIX: PrefixLen> {
    tag: u8,
    prefix_len: PREFIX,
    len: u8,
    data: *mut u8,
}

/// Leaf node: stores a u64 index into the keys/values vecs.
#[repr(C)]
#[derive(Clone, Copy)]
struct Leaf {
    tag: u8,
    payload: [u8; 15],
}

/// The tagged union. Size determined by the largest variant (INode).
#[repr(C)]
union Trie<const INLINE: usize, PREFIX: PrefixLen> {
    inode: INode<INLINE, PREFIX>,
    hnode: HNode<PREFIX>,
    leaf: Leaf,
}

// --- Tag access ---

impl<const INLINE: usize, PREFIX: PrefixLen> Trie<INLINE, PREFIX> {
    fn tag(&self) -> u8 {
        unsafe { self.leaf.tag }
    }

    fn as_inode(&self) -> Option<&INode<INLINE, PREFIX>> {
        let tag = self.tag();
        if usize::from(tag) >= 2 && usize::from(tag) <= INLINE {
            Some(unsafe { &self.inode })
        } else {
            None
        }
    }

    fn as_hnode(&self) -> Option<&HNode<PREFIX>> {
        if self.tag() == TAG_HNODE { Some(unsafe { &self.hnode }) } else { None }
    }

    fn as_leaf(&self) -> Option<&Leaf> {
        if self.tag() == TAG_LEAF { Some(unsafe { &self.leaf }) } else { None }
    }

    /// Number of children for an internal node (INode or HNode).
    /// Must not be called on a Leaf.
    fn child_count(&self) -> usize {
        match self.tag() {
            TAG_HNODE => unsafe { self.hnode }.len as usize,
            tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => tag as usize,
            _ => unreachable!("child_count called on leaf"),
        }
    }

    /// Slice of children for an internal node (INode or HNode).
    /// Must not be called on a Leaf.
    fn children_slice(&self) -> &[Trie<INLINE, PREFIX>] {
        match self.tag() {
            TAG_HNODE => unsafe { self.hnode.children::<INLINE>() },
            tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                unsafe { std::slice::from_raw_parts(self.inode.children, tag as usize) }
            }
            _ => unreachable!("children_slice called on leaf"),
        }
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

// --- HNode access helpers ---

impl<PREFIX: PrefixLen> HNode<PREFIX> {
    unsafe fn discriminants(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.len as usize) }
    }

    unsafe fn children<const INLINE: usize>(&self) -> &[Trie<INLINE, PREFIX>] {
        let trie_align = std::mem::align_of::<Trie<INLINE, PREFIX>>();
        let disc_end = align_up(self.len as usize, trie_align);
        unsafe {
            let ptr = self.data.add(disc_end) as *const Trie<INLINE, PREFIX>;
            std::slice::from_raw_parts(ptr, self.len as usize)
        }
    }

    fn find_child<const INLINE: usize>(&self, byte: u8) -> Option<usize> {
        simd::hnode_find_child(self.data, self.len as usize, byte)
    }

    /// Returns the index of the first discriminant >= `byte`,
    /// or `len` (past end) if all discriminants are < `byte`.
    fn find_child_lower_bound<const INLINE: usize>(&self, byte: u8) -> usize {
        simd::hnode_find_child_lower_bound(self.data, self.len as usize, byte)
    }
}

/// Align `val` up to the next multiple of `align` (must be a power of 2).
const fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// Free a children slice previously allocated via `Vec::into_boxed_slice()`.
///
/// Reconstructs the fat pointer from the thin `*mut Trie` and count,
/// then passes it to `Box::from_raw` for deallocation.
/// Free a children slice previously allocated via `Vec::into_boxed_slice()`.
///
/// Reconstructs the fat pointer from the thin `*mut Trie` and count,
/// then passes it to `Box::from_raw` for deallocation.
///
/// # Safety
/// `ptr` must point to a valid allocation of `count` `Trie` elements
/// previously created via `Vec::into_boxed_slice()`.
unsafe fn free_children_slice<const INLINE: usize, PREFIX: PrefixLen>(
    ptr: *mut Trie<INLINE, PREFIX>,
    count: usize,
) {
    // Safety: caller guarantees ptr is valid for count elements from a Vec::into_boxed_slice()
    let fat = std::ptr::slice_from_raw_parts_mut(ptr, count);
    unsafe { drop(Box::from_raw(fat)) };
}

/// Free an HNodeData buffer previously allocated via `alloc_hnode_data`.
///
/// # Safety
/// `ptr` must point to a valid HNodeData allocation with `len` children,
/// previously created via `alloc_hnode_data`.
unsafe fn free_hnode_data<const INLINE: usize, PREFIX: PrefixLen>(
    ptr: *mut u8,
    len: u8,
) {
    use std::alloc::{self, Layout};

    let trie_align = std::mem::align_of::<Trie<INLINE, PREFIX>>();
    let disc_end = align_up(len as usize, trie_align);
    let total = disc_end + len as usize * size_of::<Trie<INLINE, PREFIX>>();
    let layout = Layout::from_size_align(total, trie_align).unwrap();
    // Safety: ptr was allocated by alloc_hnode_data with this exact layout
    unsafe { alloc::dealloc(ptr, layout) };
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
                return unsafe { node_ref.leaf }.index() as usize;
            }
            _ => { // NOTE : we're already matching, why put children_slice on Trie instead of 
                //matching on tag to get Hnode or Inode then doing the appropriate one here? 
                // Internal node (INode or HNode) — descend to first child
                stack.push((node, 0));
                let children = node_ref.children_slice();
                node = std::ptr::from_ref(&children[0]);
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
                return unsafe { node_ref.leaf }.index() as usize;
            }
            _ => { // NOTE : same feedback as in leftmost_leaf
                // Internal node — descend to last child
                let last = node_ref.child_count() - 1;
                stack.push((node, last));
                let children = node_ref.children_slice();
                node = std::ptr::from_ref(&children[last]);
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

// Safety: the raw pointers inside Trie/INode/HNode are only dereferenced in
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

    /// Look up a key and return its index, or `None` if not found.
    /// The key should NOT include a null terminator; one is appended internally.
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        let root = self.root.as_ref()?;  //early exit if tree is empty.
        let mut nt_key = key.to_vec();
        //NOTE : check that the key doesnt have a null terminator. 
        nt_key.push(0);
        let mut node: &Trie<INLINE, PREFIX> = root;
        let mut offset = 0usize;

        loop {
            match node.tag() {
                TAG_HNODE => {
                    let h = node.as_hnode().unwrap();
                    offset += h.prefix_len.into_usize();
                    if offset >= nt_key.len() { return None; }
                    let byte = nt_key[offset];
                    offset += 1;
                    let idx = h.find_child::<INLINE>(byte)?;
                    let children = unsafe { h.children::<INLINE>() };
                    node = &children[idx];
                }
                tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                    let inode = node.as_inode().unwrap();
                    offset += inode.prefix_len.into_usize();
                    if offset >= nt_key.len() { return None; }
                    let byte = nt_key[offset];
                    offset += 1;
                    let idx = inode.find_child(byte)?;
                    let children = unsafe { std::slice::from_raw_parts(inode.children, tag as usize) };
                    node = &children[idx];
                }
                TAG_LEAF => {
                    let leaf = node.as_leaf().unwrap();
                    let index = leaf.index() as usize;
                    if index < self.keys.len() && self.keys[index] == nt_key {
                        return Some(index);
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// Insert a new key-value pair. Returns `Ok(index)` on success.
    /// Returns `Err(())` if the key already exists.
    /// Panics if the key contains a null byte (0x00).
    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        assert!(!key.contains(&0), "key must not contain null bytes");
        let mut nt_key = key;
        nt_key.push(0);

        if self.get(&nt_key[..nt_key.len() - 1]).is_some() {
            return Err(());
        }

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
                self.root = Some(self.insert_into_root(index, &nt_key));
                Ok(index)
            }
        }
    }

    fn insert_into_root(&mut self, new_index: usize, new_key: &[u8]) -> Box<Trie<INLINE, PREFIX>> {
        let old_root = self.root.take().unwrap();
        self.insert_into_node(old_root, new_key, new_index, 0)
    }

    fn insert_into_node(
        &mut self,
        node: Box<Trie<INLINE, PREFIX>>,
        new_key: &[u8],
        new_index: usize,
        offset: usize,
    ) -> Box<Trie<INLINE, PREFIX>> {
        match node.tag() {
            TAG_LEAF => {
                let leaf = unsafe { node.leaf };
                let existing_key = &self.keys[leaf.index() as usize];

                // Find where the keys diverge.
                let mut split_len = offset;
                while split_len < existing_key.len()
                    && split_len < new_key.len()
                    && existing_key[split_len] == new_key[split_len]
                {
                    split_len += 1;
                }

                let prefix_len = (split_len - offset) as u8;
                let existing_byte = existing_key[split_len];
                let new_byte = new_key[split_len];

                let existing_child = Trie { leaf: Leaf::new(leaf.index()) };
                let new_child = Trie { leaf: Leaf::new(new_index as u64) };

                let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
                    (existing_byte, existing_child, new_byte, new_child)
                } else {
                    (new_byte, new_child, existing_byte, existing_child)
                };

                let inode = make_inode_2(prefix_len, sym_a, sym_b, child_a, child_b);
                Box::new(inode)
            }

            tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                let inode = unsafe { node.inode };
                let prefix_len = inode.prefix_len.into_usize();

                // Check if new key diverges within this node's prefix.
                let existing_key = self.key_of_subtree_ptr(inode.children, tag as usize);
                for i in 0..prefix_len {
                    let ki = offset + i;
                    if ki >= new_key.len() || (ki < existing_key.len() && new_key[ki] != existing_key[ki]) {
                        // Split at this point in the prefix.
                        let new_prefix_len = i as u8;
                        let remaining_prefix = (prefix_len - i - 1) as u8;
                        let existing_byte = existing_key[ki];
                        let new_byte = if ki < new_key.len() { new_key[ki] } else { 0 };

                        let child_inode = INode {
                            tag: inode.tag,
                            prefix_len: PREFIX::from(remaining_prefix),
                            symbols: inode.symbols,
                            children: inode.children,
                        };
                        let existing_child = Trie { inode: child_inode };
                        let new_child = Trie { leaf: Leaf::new(new_index as u64) };

                        let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
                            (existing_byte, existing_child, new_byte, new_child)
                        } else {
                            (new_byte, new_child, existing_byte, existing_child)
                        };

                        let parent = make_inode_2(new_prefix_len, sym_a, sym_b, child_a, child_b);
                        return Box::new(parent);
                    }
                }

                // Key matches prefix — look up the discriminating byte.
                let byte_offset = offset + prefix_len;
                let byte = new_key[byte_offset];

                match inode.find_child(byte) {
                    Some(child_idx) => {
                        // Descend into existing child.
                        let child_count = tag as usize;
                        let old_children = unsafe {
                            std::slice::from_raw_parts(inode.children, child_count)
                        };

                        // Recursively insert into the child.
                        let old_child = unsafe { std::ptr::read(&old_children[child_idx]) };
                        let new_child_box = self.insert_into_node(
                            Box::new(old_child), new_key, new_index, byte_offset + 1,
                        );

                        // Rebuild children array with updated child.
                        let mut new_children = Vec::with_capacity(child_count);
                        for i in 0..child_count {
                            if i == child_idx {
                                new_children.push(unsafe { std::ptr::read(&*new_child_box) });
                            } else {
                                new_children.push(unsafe { std::ptr::read(&old_children[i]) });
                            }
                        }
                        let new_children_ptr = Box::into_raw(new_children.into_boxed_slice())
                            as *mut Trie<INLINE, PREFIX>;

                        unsafe { free_children_slice(inode.children, child_count); }

                        Box::new(Trie {
                            inode: INode {
                                tag: inode.tag,
                                prefix_len: inode.prefix_len,
                                symbols: inode.symbols,
                                children: new_children_ptr,
                            },
                        })
                    }
                    None => {
                        // New symbol — add a child.
                        let new_leaf = Trie { leaf: Leaf::new(new_index as u64) };
                        add_child_to_inode(inode, byte, new_leaf)
                    }
                }
            }

            TAG_HNODE => {
                let hnode = unsafe { node.hnode };
                let prefix_len = hnode.prefix_len.into_usize();

                // Check if new key diverges within this node's prefix.
                let existing_key = self.key_of_subtree_hnode(&hnode);
                for i in 0..prefix_len {
                    let ki = offset + i;
                    if ki >= new_key.len() || (ki < existing_key.len() && new_key[ki] != existing_key[ki]) {
                        let new_prefix_len = i as u8;
                        let remaining_prefix = (prefix_len - i - 1) as u8;
                        let existing_byte = existing_key[ki];
                        let new_byte = if ki < new_key.len() { new_key[ki] } else { 0 };

                        // Create an HNode child with the remaining prefix.
                        let child_hnode = HNode {
                            tag: TAG_HNODE,
                            prefix_len: PREFIX::from(remaining_prefix),
                            len: hnode.len,
                            data: hnode.data, // reuse existing allocation
                        };
                        let existing_child = Trie { hnode: child_hnode };
                        let new_child = Trie { leaf: Leaf::new(new_index as u64) };

                        let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
                            (existing_byte, existing_child, new_byte, new_child)
                        } else {
                            (new_byte, new_child, existing_byte, existing_child)
                        };

                        let parent = make_inode_2(new_prefix_len, sym_a, sym_b, child_a, child_b);
                        return Box::new(parent);
                    }
                }

                // Key matches prefix — look up the discriminating byte.
                let byte_offset = offset + prefix_len;
                let byte = new_key[byte_offset];

                match hnode.find_child::<INLINE>(byte) {
                    Some(child_idx) => {
                        // Descend into existing child.
                        let old_children = unsafe { hnode.children::<INLINE>() };
                        let old_child = unsafe { std::ptr::read(&old_children[child_idx]) };

                        let new_child_box = self.insert_into_node(
                            Box::new(old_child), new_key, new_index, byte_offset + 1,
                        );

                        // Rebuild HNodeData with updated child.
                        let old_disc = unsafe { hnode.discriminants() };
                        let mut new_children = Vec::with_capacity(hnode.len as usize);
                        for i in 0..hnode.len as usize {
                            if i == child_idx {
                                new_children.push(unsafe { std::ptr::read(&*new_child_box) });
                            } else {
                                new_children.push(unsafe { std::ptr::read(&old_children[i]) });
                            }
                        }
                        let hdata = alloc_hnode_data::<INLINE, PREFIX>(old_disc, &new_children);
                        unsafe { free_hnode_data::<INLINE, PREFIX>(hnode.data, hnode.len); }

                        Box::new(Trie {
                            hnode: HNode {
                                tag: TAG_HNODE,
                                prefix_len: hnode.prefix_len,
                                len: hnode.len,
                                data: hdata,
                            },
                        })
                    }
                    None => {
                        // New symbol — add a child.
                        let new_leaf = Trie { leaf: Leaf::new(new_index as u64) };
                        add_child_to_hnode(hnode, byte, new_leaf)
                    }
                }
            }

            _ => panic!("invalid tag in trie: {}", node.tag()),
        }
    }

    fn key_of_subtree_ptr(&self, children: *mut Trie<INLINE, PREFIX>, count: usize) -> &[u8] {
        let children = unsafe { std::slice::from_raw_parts(children, count) };
        self.key_of_subtree(&children[0])
    }

    fn key_of_subtree_hnode(&self, hnode: &HNode<PREFIX>) -> &[u8] {
        let children = unsafe { hnode.children::<INLINE>() };
        self.key_of_subtree(&children[0])
    }

    fn key_of_subtree(&self, node: &Trie<INLINE, PREFIX>) -> &[u8] {
        let mut node = node;
        loop {
            match node.tag() {
                TAG_LEAF => {
                    let index = unsafe { node.leaf }.index() as usize;
                    return &self.keys[index];
                }
                tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                    let inode = unsafe { &node.inode };
                    let children = unsafe { std::slice::from_raw_parts(inode.children, tag as usize) };
                    node = &children[0];
                }
                TAG_HNODE => {
                    let h = unsafe { &node.hnode };
                    let children = unsafe { h.children::<INLINE>() };
                    node = &children[0];
                }
                _ => panic!("invalid tag"),
            }
        }
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
        // freeing the root node's 16 bytes. Since Trie is Copy with no Drop,
        // the Box<Trie> doesn't follow the (now-dangling) children/data pointers.
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
            tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                let inode = node.as_inode().unwrap();
                let count = tag as usize;
                // Safety: inode.children is valid for count elements
                let children = unsafe { std::slice::from_raw_parts(inode.children, count) };
                for child in children {
                    unsafe { Self::free_subtree(child) };
                }
                unsafe { free_children_slice(inode.children, count) };
            }
            TAG_HNODE => {
                let hnode = node.as_hnode().unwrap();
                // Safety: hnode.data is valid HNodeData with len children
                let children = unsafe { hnode.children::<INLINE>() };
                for child in children {
                    unsafe { Self::free_subtree(child) };
                }
                unsafe { free_hnode_data::<INLINE, PREFIX>(hnode.data, hnode.len) };
            }
            _ => {}
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
                let children = node_ref.children_slice();
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
                let children = node_ref.children_slice();
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
    /// The key should NOT include a null terminator; one is appended internally.
    /// If no such key exists, the iterator becomes exhausted.
    pub fn seek(&mut self, key: &[u8]) {
        let mut nt_key = key.to_vec();
        nt_key.push(0);
        self.stack.clear();
        self.current = None;

        let Some(root) = &self.trie.root else { return };
        let mut node: *const Trie<INLINE, PREFIX> = &**root;
        let mut offset = 0usize;

        loop {
            let node_ref = unsafe { &*node };

            match node_ref.tag() {
                TAG_LEAF => {
                    let leaf_idx = unsafe { node_ref.leaf }.index() as usize;
                    self.current = Some(leaf_idx);
                    // If leaf key < seek key, advance forward
                    if self.trie.keys[leaf_idx].as_slice() < nt_key.as_slice() {
                        self.next();
                    }
                    return;
                }

                tag if usize::from(tag) >= 2 && usize::from(tag) <= INLINE => {
                    let inode = node_ref.as_inode().unwrap();
                    offset += inode.prefix_len.into_usize();

                    if offset >= nt_key.len() {
                        // Seek key exhausted — leftmost leaf is >= seek key
                        self.stack.push((node, 0));
                        let children = unsafe {
                            std::slice::from_raw_parts(inode.children, tag as usize)
                        };
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&children[0]), &mut self.stack,
                        ));
                        return;
                    }

                    let byte = nt_key[offset];
                    offset += 1;
                    let lb = inode.find_child_lower_bound(byte);

                    if lb < tag as usize && inode.symbols[lb] == byte {
                        // Exact match — descend
                        self.stack.push((node, lb));
                        node = unsafe { inode.children.add(lb) };
                    } else if lb < tag as usize {
                        // First child > byte — its leftmost leaf is >= seek key
                        self.stack.push((node, lb));
                        let children = unsafe {
                            std::slice::from_raw_parts(inode.children, tag as usize)
                        };
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&children[lb]), &mut self.stack,
                        ));
                        return;
                    } else {
                        // All children < byte — push sentinel, advance via next()
                        self.stack.push((node, tag as usize));
                        self.next();
                        return;
                    }
                }

                TAG_HNODE => {
                    let hnode = node_ref.as_hnode().unwrap();
                    offset += hnode.prefix_len.into_usize();

                    if offset >= nt_key.len() {
                        self.stack.push((node, 0));
                        let children = unsafe { hnode.children::<INLINE>() };
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&children[0]), &mut self.stack,
                        ));
                        return;
                    }

                    let byte = nt_key[offset];
                    offset += 1;
                    let lb = hnode.find_child_lower_bound::<INLINE>(byte);

                    if lb < hnode.len as usize && unsafe { hnode.discriminants()[lb] } == byte {
                        // Exact match — descend
                        self.stack.push((node, lb));
                        let children = unsafe { hnode.children::<INLINE>() };
                        node = std::ptr::from_ref(&children[lb]);
                    } else if lb < hnode.len as usize {
                        // First child > byte — its leftmost leaf is >= seek key
                        self.stack.push((node, lb));
                        let children = unsafe { hnode.children::<INLINE>() };
                        self.current = Some(leftmost_leaf(
                            std::ptr::from_ref(&children[lb]), &mut self.stack,
                        ));
                        return;
                    } else {
                        // All children < byte — push sentinel, advance via next()
                        self.stack.push((node, hnode.len as usize));
                        self.next();
                        return;
                    }
                }

                _ => return,
            }
        }
    }
}

// --- Free functions ---

fn make_inode_2<const INLINE: usize, PREFIX: PrefixLen>(
    prefix_len: u8,
    sym_a: u8,
    sym_b: u8,
    child_a: Trie<INLINE, PREFIX>,
    child_b: Trie<INLINE, PREFIX>,
) -> Trie<INLINE, PREFIX> {
    let mut symbols = [0u8; INLINE];
    symbols[0] = sym_a;
    symbols[1] = sym_b;
    let children = vec![child_a, child_b];
    let children_ptr = Box::into_raw(children.into_boxed_slice()) as *mut Trie<INLINE, PREFIX>;
    Trie {
        inode: INode {
            tag: 2,
            prefix_len: PREFIX::from(prefix_len),
            symbols,
            children: children_ptr,
        },
    }
}

fn add_child_to_inode<const INLINE: usize, PREFIX: PrefixLen>(
    inode: INode<INLINE, PREFIX>,
    byte: u8,
    new_child: Trie<INLINE, PREFIX>,
) -> Box<Trie<INLINE, PREFIX>> {
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

        Box::new(Trie {
            inode: INode {
                tag: new_tag,
                prefix_len: inode.prefix_len,
                symbols,
                children: new_children_ptr,
            },
        })
    } else {
        // INode is full — promote to HNode.
        promote_inode_to_hnode(inode, byte, new_child)
    }
}

/// Promote an INode (with INLINE children) to an HNode (with INLINE+1 children).
fn promote_inode_to_hnode<const INLINE: usize, PREFIX: PrefixLen>(
    inode: INode<INLINE, PREFIX>,
    new_byte: u8,
    new_child: Trie<INLINE, PREFIX>,
) -> Box<Trie<INLINE, PREFIX>> {
    let old_count = INLINE; // INode is full, so it has exactly INLINE children
    let new_count = old_count + 1;

    // Find insertion position for the new symbol.
    let insert_pos = inode.symbols[..old_count]
        .iter().position(|&s| s > new_byte).unwrap_or(old_count);

    // Build merged discriminants array.
    let mut disc = Vec::with_capacity(new_count);
    for i in 0..insert_pos {
        disc.push(inode.symbols[i]);
    }
    disc.push(new_byte);
    for i in insert_pos..old_count {
        disc.push(inode.symbols[i]);
    }

    // Build merged children array.
    let old_children = unsafe { std::slice::from_raw_parts(inode.children, old_count) };
    let mut new_children = Vec::with_capacity(new_count);
    for i in 0..insert_pos {
        new_children.push(unsafe { std::ptr::read(&old_children[i]) });
    }
    new_children.push(new_child);
    for i in insert_pos..old_count {
        new_children.push(unsafe { std::ptr::read(&old_children[i]) });
    }

    // Allocate HNodeData: [discriminants] [padding] [children]
    let hdata = alloc_hnode_data(&disc, &new_children);

    // Free the old inline children slice.
    unsafe { free_children_slice(inode.children, old_count); }

    Box::new(Trie {
        hnode: HNode {
            tag: TAG_HNODE,
            prefix_len: inode.prefix_len,
            len: new_count as u8,
            data: hdata,
        },
    })
}

/// Allocate contiguous HNodeData: discriminants + padding + children.
/// Returns a pointer suitable for `HNode.data`.
///
/// The allocation uses the alignment of `Trie` so that the children section
/// (starting at `align_up(len, align_of::<Trie>())`) is properly aligned.
fn alloc_hnode_data<const INLINE: usize, PREFIX: PrefixLen>(
    discriminants: &[u8],
    children: &[Trie<INLINE, PREFIX>],
) -> *mut u8 {
    use std::alloc::{self, Layout};

    let len = discriminants.len();
    let trie_align = std::mem::align_of::<Trie<INLINE, PREFIX>>();
    let disc_end = align_up(len, trie_align);
    let children_size = children.len() * size_of::<Trie<INLINE, PREFIX>>();
    let total = disc_end + children_size;

    let layout = Layout::from_size_align(total, trie_align).unwrap();
    // Safety: layout has non-zero size (len >= 7 for HNode)
    let ptr = unsafe { alloc::alloc(layout) };

    // Write discriminants
    unsafe { std::ptr::copy_nonoverlapping(discriminants.as_ptr(), ptr, len) };
    // Zero padding
    unsafe { std::ptr::write_bytes(ptr.add(len), 0, disc_end - len) };
    // Write children
    unsafe {
        std::ptr::copy_nonoverlapping(
            children.as_ptr() as *const u8,
            ptr.add(disc_end),
            children_size,
        );
    }

    ptr
}

/// Add a child to an HNode. Returns the new HNode.
fn add_child_to_hnode<const INLINE: usize, PREFIX: PrefixLen>(
    hnode: HNode<PREFIX>,
    byte: u8,
    new_child: Trie<INLINE, PREFIX>,
) -> Box<Trie<INLINE, PREFIX>> {
    let old_len = hnode.len as usize;
    let new_len = old_len + 1;

    let old_disc = unsafe { hnode.discriminants() };
    let old_children = unsafe { hnode.children::<INLINE>() };

    // Find insertion position.
    let insert_pos = old_disc.binary_search(&byte).unwrap_or_else(|p| p);

    // Build merged discriminants.
    let mut disc = Vec::with_capacity(new_len);
    disc.extend_from_slice(&old_disc[..insert_pos]);
    disc.push(byte);
    disc.extend_from_slice(&old_disc[insert_pos..]);

    // Build merged children.
    let mut new_children = Vec::with_capacity(new_len);
    for i in 0..insert_pos {
        new_children.push(unsafe { std::ptr::read(&old_children[i]) });
    }
    new_children.push(new_child);
    for i in insert_pos..old_len {
        new_children.push(unsafe { std::ptr::read(&old_children[i]) });
    }

    let hdata = alloc_hnode_data::<INLINE, PREFIX>(&disc, &new_children);

    unsafe { free_hnode_data::<INLINE, PREFIX>(hnode.data, hnode.len); }

    Box::new(Trie {
        hnode: HNode {
            tag: TAG_HNODE,
            prefix_len: hnode.prefix_len,
            len: new_len as u8,
            data: hdata,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    const fn align_up(val: usize, align: usize) -> usize {
        (val + align - 1) & !(align - 1)
    }

    pub const fn compute_node_size(symbols_len: usize, prefix_len_size: usize, prefix_len_align: usize) -> usize {
        let p_offset = align_up(1, prefix_len_align);
        let symbols_end = p_offset + prefix_len_size + symbols_len;
        let children_offset = align_up(symbols_end, 8);
        children_offset + 8
    }

    #[test]
    fn default_node_is_16_bytes() {
        assert_eq!(size_of::<INode<6, u8>>(), 16);
        assert_eq!(size_of::<HNode<u8>>(), 16);
        assert_eq!(size_of::<Leaf>(), 16);
        assert_eq!(size_of::<Trie<6, u8>>(), 16);
    }

    #[test]
    fn inode_layout_offsets() {
        assert_eq!(std::mem::offset_of!(INode<6, u8>, tag), 0);
        assert_eq!(std::mem::offset_of!(INode<6, u8>, prefix_len), 1);
        assert_eq!(std::mem::offset_of!(INode<6, u8>, symbols), 2);
        assert_eq!(std::mem::offset_of!(INode<6, u8>, children), 8);
    }

    #[test]
    fn hnode_layout_offsets() {
        assert_eq!(std::mem::offset_of!(HNode<u8>, tag), 0);
        assert_eq!(std::mem::offset_of!(HNode<u8>, prefix_len), 1);
        assert_eq!(std::mem::offset_of!(HNode<u8>, len), 2);
        assert_eq!(std::mem::offset_of!(HNode<u8>, data), 8);
    }

    #[test]
    fn leaf_layout_offsets() {
        assert_eq!(std::mem::offset_of!(Leaf, tag), 0);
        assert_eq!(std::mem::offset_of!(Leaf, payload), 1);
    }

    #[test]
    fn u16_prefix_node_is_24_bytes() {
        assert_eq!(size_of::<INode<6, u16>>(), 24);
        assert_eq!(size_of::<Trie<6, u16>>(), 24);
    }

    #[test]
    fn dense_inline_node_is_24_bytes() {
        assert_eq!(size_of::<INode<14, u8>>(), 24);
        assert_eq!(size_of::<Trie<14, u8>>(), 24);
    }

    #[test]
    fn compute_node_size_matches_inode() {
        assert_eq!(compute_node_size(6, 1, 1), size_of::<INode<6, u8>>());
        assert_eq!(compute_node_size(6, 2, 2), size_of::<INode<6, u16>>());
        assert_eq!(compute_node_size(14, 1, 1), size_of::<INode<14, u8>>());
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        let idx = trie.insert(b"hello".to_vec(), "world").unwrap();
        assert_eq!(idx, 0);
        assert_eq!(trie.get(b"hello"), Some(0));
        assert_eq!(trie.get(b"world"), None);
    }

    #[test]
    fn insert_duplicate_returns_error() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"hello".to_vec(), "world").unwrap();
        assert!(trie.insert(b"hello".to_vec(), "other").is_err());
    }

    #[test]
    fn insert_rejects_null_byte() {
        let mut trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = trie.insert(b"hel\x00lo".to_vec(), 42);
        }));
        assert!(result.is_err());
    }

    #[test]
    fn insert_two_keys_split_leaf() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), "first").unwrap();
        trie.insert(b"abd".to_vec(), "second").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"abd"), Some(1));
        assert_eq!(trie.get(b"abe"), None);
        assert_eq!(trie.get(b"ab"), None);
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), "1").unwrap();
        trie.insert(b"abd".to_vec(), "2").unwrap();
        trie.insert(b"abe".to_vec(), "3").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"abd"), Some(1));
        assert_eq!(trie.get(b"abe"), Some(2));
        assert_eq!(trie.get(b"abf"), None);
    }

    #[test]
    fn insert_prefix_key() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), "long").unwrap();
        trie.insert(b"ab".to_vec(), "short").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"ab"), Some(1));
        assert_eq!(trie.get(b"abd"), None);
    }

    #[test]
    fn insert_reverse_prefix_key() {
        // Insert short key first, then long key.
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"ab".to_vec(), "short").unwrap();
        trie.insert(b"abc".to_vec(), "long").unwrap();
        assert_eq!(trie.get(b"ab"), Some(0));
        assert_eq!(trie.get(b"abc"), Some(1));
    }

    #[test]
    fn insert_no_common_prefix() {
        // Keys with no common prefix.
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), "1").unwrap();
        trie.insert(b"xyz".to_vec(), "2").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"xyz"), Some(1));
        assert_eq!(trie.get(b"ab"), None);
        assert_eq!(trie.get(b"abcz"), None);
    }

    #[test]
    fn insert_single_char_keys() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"a".to_vec(), "1").unwrap();
        trie.insert(b"b".to_vec(), "2").unwrap();
        trie.insert(b"c".to_vec(), "3").unwrap();
        assert_eq!(trie.get(b"a"), Some(0));
        assert_eq!(trie.get(b"b"), Some(1));
        assert_eq!(trie.get(b"c"), Some(2));
        assert_eq!(trie.get(b"d"), None);
    }

    #[test]
    fn insert_many_keys_same_prefix() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        for i in 0u8..6 {
            let mut key = b"prefix".to_vec();
            key.push(b'a' + i);
            trie.insert(key, i as usize).unwrap();
        }
        assert_eq!(trie.get(b"prefixa"), Some(0));
        assert_eq!(trie.get(b"prefixb"), Some(1));
        assert_eq!(trie.get(b"prefixf"), Some(5));
        assert_eq!(trie.get(b"prefixg"), None);
        assert_eq!(trie.get(b"prefi"), None);
    }

    #[test]
    fn insert_deeply_nested() {
        // Insert keys that create a chain of splits.
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"a".to_vec(), 0).unwrap();
        trie.insert(b"ab".to_vec(), 1).unwrap();
        trie.insert(b"abc".to_vec(), 2).unwrap();
        trie.insert(b"abcd".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"a"), Some(0));
        assert_eq!(trie.get(b"ab"), Some(1));
        assert_eq!(trie.get(b"abc"), Some(2));
        assert_eq!(trie.get(b"abcd"), Some(3));
        assert_eq!(trie.get(b"abcde"), None);
        assert_eq!(trie.get(b"b"), None);
    }

    #[test]
    fn insert_branching_at_root() {
        // Keys that diverge at the first byte (no shared prefix).
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"aaa".to_vec(), 0).unwrap();
        trie.insert(b"bbb".to_vec(), 1).unwrap();
        trie.insert(b"ccc".to_vec(), 2).unwrap();
        assert_eq!(trie.get(b"aaa"), Some(0));
        assert_eq!(trie.get(b"bbb"), Some(1));
        assert_eq!(trie.get(b"ccc"), Some(2));
        assert_eq!(trie.get(b"ddd"), None);
        assert_eq!(trie.get(b"aab"), None);
    }

    #[test]
    fn insert_longer_after_shorter() {
        // "ab" then "abcd" — extending beyond an existing prefix.
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"ab".to_vec(), 0).unwrap();
        trie.insert(b"abcd".to_vec(), 1).unwrap();
        assert_eq!(trie.get(b"ab"), Some(0));
        assert_eq!(trie.get(b"abcd"), Some(1));
        assert_eq!(trie.get(b"abc"), None);
        assert_eq!(trie.get(b"abcde"), None);
    }

    #[test]
    fn insert_promotes_inode_to_hnode() {
        // With INLINE=4, the 5th child triggers promotion to HNode.
        let mut trie: TinyTrie<usize, 4, u8> = TinyTrie::new();
        for i in 0u8..7 {
            let mut key = b"prefix".to_vec();
            key.push(b'a' + i);
            trie.insert(key, i as usize).unwrap();
        }
        // All 7 should be findable.
        for i in 0u8..7 {
            let mut key = b"prefix".to_vec();
            key.push(b'a' + i);
            assert_eq!(trie.get(&key), Some(i as usize));
        }
        assert_eq!(trie.get(b"prefixh"), None);
    }

    #[test]
    fn insert_many_keys_exhausts_inline() {
        // Insert 20 keys with the same prefix (INLINE=6, so HNode after 6).
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        for i in 0..20 {
            let key = format!("key{:02}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        for i in 0..20 {
            let key = format!("key{:02}", i);
            assert_eq!(trie.get(key.as_bytes()), Some(i));
        }
        assert_eq!(trie.get(b"key20"), None);
        assert_eq!(trie.get(b"key"), None);
    }

    // --- Iterator tests ---

    #[test]
    fn iter_empty() {
        let trie: TinyTrie<i32, 6, u8> = TinyTrie::new();
        let mut iter = trie.iter();
        assert!(iter.current().is_none());
        assert!(iter.next().is_none());
        assert!(iter.prev().is_none());
    }

    #[test]
    fn iter_single_key() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"hello".to_vec(), "world").unwrap();
        let mut iter = trie.iter();
        assert_eq!(iter.current(), Some((b"hello".as_slice(), &"world")));
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_forward() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"b".to_vec(), "2").unwrap();
        trie.insert(b"d".to_vec(), "4").unwrap();
        trie.insert(b"f".to_vec(), "6").unwrap();
        let mut iter = trie.iter();
        assert_eq!(iter.current(), Some((b"b".as_slice(), &"2")));
        assert_eq!(iter.next(), Some((b"d".as_slice(), &"4")));
        assert_eq!(iter.next(), Some((b"f".as_slice(), &"6")));
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_backward() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"b".to_vec(), "2").unwrap();
        trie.insert(b"d".to_vec(), "4").unwrap();
        trie.insert(b"f".to_vec(), "6").unwrap();
        let mut iter = trie.iter();
        // Advance to last key
        iter.next(); // d
        iter.next(); // f
        assert_eq!(iter.current(), Some((b"f".as_slice(), &"6")));
        assert_eq!(iter.prev(), Some((b"d".as_slice(), &"4")));
        assert_eq!(iter.prev(), Some((b"b".as_slice(), &"2")));
        assert!(iter.prev().is_none());
    }

    #[test]
    fn iter_prev_before_first() {
        let mut trie: TinyTrie<&str, 6, u8> = TinyTrie::new();
        trie.insert(b"a".to_vec(), "1").unwrap();
        let mut iter = trie.iter();
        assert_eq!(iter.current(), Some((b"a".as_slice(), &"1")));
        assert!(iter.prev().is_none());
    }

    #[test]
    fn iter_seek_exact() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), 0).unwrap();
        trie.insert(b"abd".to_vec(), 1).unwrap();
        trie.insert(b"xyz".to_vec(), 2).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"abd");
        assert_eq!(iter.current(), Some((b"abd".as_slice(), &1)));
    }

    #[test]
    fn iter_seek_between() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"abc".to_vec(), 0).unwrap();
        trie.insert(b"xyz".to_vec(), 1).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"mno");
        assert_eq!(iter.current(), Some((b"xyz".as_slice(), &1)));
    }

    #[test]
    fn iter_seek_before_all() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"bbb".to_vec(), 0).unwrap();
        trie.insert(b"ccc".to_vec(), 1).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"aaa");
        assert_eq!(iter.current(), Some((b"bbb".as_slice(), &0)));
    }

    #[test]
    fn iter_seek_after_all() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"bbb".to_vec(), 0).unwrap();
        trie.insert(b"yyy".to_vec(), 1).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"zzz");
        assert!(iter.current().is_none());
    }

    #[test]
    fn iter_seek_prefix_key() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"ab".to_vec(), 0).unwrap();
        trie.insert(b"abc".to_vec(), 1).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"ab");
        assert_eq!(iter.current(), Some((b"ab".as_slice(), &0)));
    }

    #[test]
    fn iter_seek_prefix_longer() {
        // Seek "abc" when trie has "ab" and "abcd"
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"ab".to_vec(), 0).unwrap();
        trie.insert(b"abcd".to_vec(), 1).unwrap();
        let mut iter = trie.iter();
        iter.seek(b"abc");
        assert_eq!(iter.current(), Some((b"abcd".as_slice(), &1)));
    }

    #[test]
    fn iter_seek_then_iterate() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        for i in 0..10 {
            let key = format!("key{:02}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }
        let mut iter = trie.iter();
        iter.seek(b"key05");
        assert_eq!(iter.current(), Some((b"key05".as_slice(), &5)));
        // next should go to key06
        assert_eq!(iter.next(), Some((b"key06".as_slice(), &6)));
        // prev should go back to key05
        assert_eq!(iter.prev(), Some((b"key05".as_slice(), &5)));
    }

    #[test]
    fn iter_hnode() {
        // Force HNode by inserting >INLINE keys with same prefix (INLINE=6)
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        for i in 0u8..10 {
            let mut key = b"prefix".to_vec();
            key.push(b'a' + i);
            trie.insert(key, i as usize).unwrap();
        }
        // Forward iteration through HNode
        let mut iter = trie.iter();
        for i in 0u8..10 {
            let mut expected_key = b"prefix".to_vec();
            expected_key.push(b'a' + i);
            assert_eq!(iter.current(), Some((expected_key.as_slice(), &(i as usize))));
            if i < 9 {
                iter.next();
            }
        }
        assert!(iter.next().is_none());

        // Backward iteration through HNode
        let mut iter2 = trie.iter();
        iter2.seek(b"prefixj"); // last key
        for i in (0..10).rev() {
            let mut expected_key = b"prefix".to_vec();
            expected_key.push(b'a' + i as u8);
            assert_eq!(iter2.current(), Some((expected_key.as_slice(), &(i as usize))));
            if i > 0 {
                iter2.prev();
            }
        }
        assert!(iter2.prev().is_none());
    }

    #[test]
    fn iter_deeply_nested() {
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        trie.insert(b"a".to_vec(), 0).unwrap();
        trie.insert(b"ab".to_vec(), 1).unwrap();
        trie.insert(b"abc".to_vec(), 2).unwrap();
        trie.insert(b"abcd".to_vec(), 3).unwrap();
        // Forward
        let mut iter = trie.iter();
        assert_eq!(iter.current(), Some((b"a".as_slice(), &0)));
        assert_eq!(iter.next(), Some((b"ab".as_slice(), &1)));
        assert_eq!(iter.next(), Some((b"abc".as_slice(), &2)));
        assert_eq!(iter.next(), Some((b"abcd".as_slice(), &3)));
        assert!(iter.next().is_none());
        // Backward
        assert_eq!(iter.prev(), Some((b"abc".as_slice(), &2)));
        assert_eq!(iter.prev(), Some((b"ab".as_slice(), &1)));
        assert_eq!(iter.prev(), Some((b"a".as_slice(), &0)));
        assert!(iter.prev().is_none());
    }

    #[test]
    fn iter_full_sort_order() {
        // Insert keys in random order, verify iteration returns them sorted
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        let keys: Vec<Vec<u8>> = vec![
            b"delta".to_vec(), b"alpha".to_vec(), b"charlie".to_vec(),
            b"bravo".to_vec(), b"echo".to_vec(),
        ];
        for (i, k) in keys.iter().enumerate() {
            trie.insert(k.clone(), i).unwrap();
        }
        let mut iter = trie.iter();
        let mut collected: Vec<Vec<u8>> = Vec::new();
        loop {
            if let Some((k, _)) = iter.current() {
                collected.push(k.to_vec());
            }
            if iter.next().is_none() { break; }
        }
        let expected: Vec<&[u8]> = vec![b"alpha", b"bravo", b"charlie", b"delta", b"echo"];
        assert_eq!(collected, expected);
    }

    #[test]
    fn iter_many_keys_forward() {
        // Insert 200 keys and iterate all of them forward to test for hangs
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        let n = 200;
        for i in 0..n {
            let key = format!("key_{:04}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }

        let mut iter = trie.iter();
        let mut count = 0;
        let mut last_key: Option<Vec<u8>> = None;

        loop {
            if let Some((k, v)) = iter.current() {
                // Verify iteration is in sorted order
                if let Some(ref prev) = last_key {
                    assert!(k > prev.as_slice(), "iteration not in sorted order: {:?} <= {:?}", k, prev);
                }
                assert_eq!(*v, count, "value mismatch at key {:?}", k);
                last_key = Some(k.to_vec());
                count += 1;
            } else {
                // current() returned None before next() — only valid if exhausted
            }
            if iter.next().is_none() {
                break;
            }
            // Safety valve: if we iterate more than n times, something is wrong
            assert!(count <= n, "iterated more than {} times, likely infinite loop", n);
        }

        assert_eq!(count, n, "expected {} iterations, got {}", n, count);
    }

    #[test]
    fn iter_many_keys_backward() {
        // Insert 200 keys, seek to end, iterate backward
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        let n = 200;
        for i in 0..n {
            let key = format!("key_{:04}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }

        // Seek to the last key
        let mut iter = trie.iter();
        iter.seek(&format!("key_{:04}", n - 1).into_bytes());
        assert_eq!(iter.current().map(|(k, _)| k.to_vec()), Some(format!("key_{:04}", n - 1).into_bytes()));

        // Iterate backward
        let mut count = 1; // already at the last key
        loop {
            if iter.prev().is_none() {
                break;
            }
            count += 1;
            assert!(count <= n, "iterated more than {} times backward, likely infinite loop", n);
        }
        assert_eq!(count, n, "expected {} backward iterations, got {}", n, count);
    }

    #[test]
    fn iter_seek_and_scan_forward() {
        // Insert 100 keys, seek to middle, scan forward to end
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        let n = 100;
        for i in 0..n {
            let key = format!("key_{:04}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }

        let mut iter = trie.iter();
        let start = 50;
        iter.seek(&format!("key_{:04}", start).into_bytes());
        assert_eq!(iter.current(), Some((format!("key_{:04}", start).as_bytes(), &start)));

        let mut count = 0;
        let mut expected = start;
        loop {
            if let Some((k, v)) = iter.current() {
                assert_eq!(k, format!("key_{:04}", expected).as_bytes());
                assert_eq!(*v, expected);
                count += 1;
                expected += 1;
            }
            if iter.next().is_none() { break; }
            assert!(count <= n - start, "too many iterations");
        }
        assert_eq!(count, n - start);
    }

    #[test]
    fn iter_last_and_backward() {
        // iter_last() positions at the last key, then iterate backward
        let mut trie: TinyTrie<usize, 6, u8> = TinyTrie::new();
        let n = 50;
        for i in 0..n {
            let key = format!("key_{:04}", i);
            trie.insert(key.into_bytes(), i).unwrap();
        }

        let mut iter = trie.iter_last();
        assert_eq!(iter.current(), Some((format!("key_{:04}", n - 1).as_bytes(), &(n - 1))));

        // Iterate backward
        let mut count = 1; // already at last key
        while iter.prev().is_some() {
            count += 1;
            assert!(count <= n, "too many backward iterations");
        }
        assert_eq!(count, n, "expected {} iterations, got {}", n, count);
    }
}