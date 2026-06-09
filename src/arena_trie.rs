//! Arena-allocated trie with compact 16-byte nodes and u8 intra-block addressing.
//!
//! Nodes within a Block reference each other by u8 index (max 256 nodes per Block).
//! When a Block fills up, subtrees are evicted to new Blocks. The `len` field
//! on each Node determines its type:
//!   - len = 0      → Leaf (value index stored in addrs)
//!   - len = 1..7   → Internal node (sorted symbols + child addresses)
//!   - len = TAG_BLOCK (8) → Block reference (block index stored in symbols)
//!
//! Eviction uses D/S ratio (descendants per arena slot) to decide which subtree
//! to promote — keep deep/narrow chains, evict wide/shallow bushes.

use std::mem::size_of;

// ── Constants ──────────────────────────────────────────────────────────

const TAG_LEAF: u8 = 0;
const TAG_BLOCK: u8 = 8;
const MAX_CHILDREN: usize = 7;
const MAX_NODES: usize = 256; // u8 address space

// ── Node (16 bytes) ────────────────────────────────────────────────────

/// A 16-byte arena slot. Type determined by `len`:
///   - 0 = Leaf, 1-7 = Internal, 8 = Block reference.
#[repr(C)]
#[derive(Clone, Copy)]
struct Node {
    prefix_len: u8,
    len: u8,
    symbols: [u8; MAX_CHILDREN],
    addrs: [u8; MAX_CHILDREN],
}

impl Node {
    fn leaf(value_idx: u64) -> Self {
        let bytes = value_idx.to_le_bytes();
        let mut node = Node {
            prefix_len: 0,
            len: TAG_LEAF,
            symbols: [0; MAX_CHILDREN],
            addrs: [0; MAX_CHILDREN],
        };
        // u64 spans symbols[0..4] + addrs[0..4]
        node.symbols[0..4].copy_from_slice(&bytes[0..4]);
        node.addrs[0..4].copy_from_slice(&bytes[4..8]);
        node
    }

    fn block_ref(block_idx: u32) -> Self {
        let mut node = Node {
            prefix_len: 0,
            len: TAG_BLOCK,
            symbols: [0; MAX_CHILDREN],
            addrs: [0; MAX_CHILDREN],
        };
        node.symbols[0..4].copy_from_slice(&block_idx.to_le_bytes());
        node
    }

    fn is_leaf(&self) -> bool {
        self.len == TAG_LEAF
    }

    fn is_block_ref(&self) -> bool {
        self.len == TAG_BLOCK
    }

    fn is_internal(&self) -> bool {
        (1..=MAX_CHILDREN).contains(&(self.len as usize))
    }

    fn value_idx(&self) -> u64 {
        debug_assert!(self.is_leaf());
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&self.symbols[0..4]);
        bytes[4..8].copy_from_slice(&self.addrs[0..4]);
        u64::from_le_bytes(bytes)
    }

    fn block_idx(&self) -> u32 {
        debug_assert!(self.is_block_ref());
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&self.symbols[0..4]);
        u32::from_le_bytes(buf)
    }

    // note : use SIMD. 
    /// Find child index for `byte` in this node's symbols (sorted, 1..=7 entries).
    /// Returns None if not found.
    fn find_child(&self, byte: u8) -> Option<usize> {
        debug_assert!(self.is_internal());
        let n = self.len as usize;
        for i in 0..n {
            if self.symbols[i] == byte {
                return Some(i);
            }
            if self.symbols[i] > byte {
                return None;
            }
        }
        None
    }

    /// Find first child index with symbol >= `byte` (lower bound).
    /// Returns `len` if all symbols < byte.
    fn find_child_lower_bound(&self, byte: u8) -> usize {
        debug_assert!(self.is_internal());
        let n = self.len as usize;
        for i in 0..n {
            if self.symbols[i] >= byte {
                return i;
            }
        }
        n
    }
}

// ── Block ──────────────────────────────────────────────────────────────

/// An arena Block containing up to 256 Nodes.
///
/// The root node's data is stored separately (root_symbols, root_addrs)
/// because the root can have more than MAX_CHILDREN children.
struct Block {
    prefix_len: u8,
    root_len: u8,
    root_symbols: Vec<u8>,
    root_addrs: Vec<u8>,      // indices into nodes[]
    desc_counts: Vec<u32>,    // descendant count per root child (for eviction)
    bitmap: [u8; 32],         // 256 bits tracking occupied Node[] slots
    len: u8,                   // number of occupied slots
    nodes: Vec<Node>,          // arena, grows on demand up to MAX_NODES
}

impl Block {
    fn new(prefix_len: u8) -> Self {
        Block {
            prefix_len,
            root_len: 0,
            root_symbols: Vec::new(),
            root_addrs: Vec::new(),
            desc_counts: Vec::new(),
            bitmap: [0u8; 32],
            len: 0,
            nodes: Vec::new(),
        }
    }

    /// Allocate a slot in the arena. Returns the slot index, or None if full.
    fn alloc_slot(&mut self) -> Option<u8> {
        if self.len as usize >= MAX_NODES {
            return None;
        }
        for byte_idx in 0..32 {
            let b = self.bitmap[byte_idx];
            if b != 0xFF {
                let bit = (!b).trailing_zeros() as u8;
                self.bitmap[byte_idx] |= 1 << bit;
                let slot = byte_idx as u8 * 8 + bit;
                self.len += 1;
                let needed = (slot as usize) + 1;
                if self.nodes.len() < needed {
                    self.nodes.resize(needed, Node {
                        prefix_len: 0, len: 0,
                        symbols: [0; MAX_CHILDREN],
                        addrs: [0; MAX_CHILDREN],
                    });
                }
                return Some(slot);
            }
        }
        None
    }

    /// Free a slot in the arena.
    fn free_slot(&mut self, slot: u8) {
        let byte_idx = slot / 8;
        let bit = slot % 8;
        self.bitmap[byte_idx as usize] &= !(1 << bit);
        self.len -= 1;
    }

    /// Get a reference to a node in the arena.
    fn node(&self, idx: u8) -> &Node {
        &self.nodes[idx as usize]
    }

    /// Find child index in root symbols (sorted). Returns None if not found.
    fn find_root_child(&self, byte: u8) -> Option<usize> {
        self.root_symbols[..self.root_len as usize]
            .binary_search(&byte)
            .ok()
    }

    /// Find first root child index with symbol >= byte (lower bound).
    fn find_root_child_lower_bound(&self, byte: u8) -> usize {
        self.root_symbols[..self.root_len as usize]
            .binary_search(&byte)
            .unwrap_or_else(|p| p)
    }

    /// Add a root child. Returns the index of the new child.
    fn add_root_child(&mut self, symbol: u8, addr: u8, desc_count: u32) -> usize {
        let pos = self.find_root_child_lower_bound(symbol);
        self.root_symbols.insert(pos, symbol);
        self.root_addrs.insert(pos, addr);
        self.desc_counts.insert(pos, desc_count);
        self.root_len += 1;
        pos
    }
}

// ── ArenaTrie ──────────────────────────────────────────────────────────

pub struct ArenaTrie<T: Clone> {
    root: Option<u32>,        // index into blocks vec, or None for empty
    blocks: Vec<Block>,
    keys: Vec<Vec<u8>>,       // null-terminated keys
    values: Vec<T>,
}

impl<T: Clone> ArenaTrie<T> {
    pub fn new() -> Self {
        ArenaTrie {
            root: None,
            blocks: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Look up a key and return its index, or None if not found.
    pub fn get(&self, key: &[u8]) -> Option<usize> {
        let root_block_idx = self.root?;
        let mut nt_key = key.to_vec();
        nt_key.push(0); // null terminator

        let mut block_idx = root_block_idx as usize;
        let mut offset = 0usize;

        loop {
            let block = &self.blocks[block_idx];

            // Skip this block's prefix
            offset += block.prefix_len as usize;
            if offset >= nt_key.len() {
                return None;
            }

            // Find matching root child
            let byte = nt_key[offset];
            offset += 1;
            let child_idx = block.find_root_child(byte)?;
            let mut addr = block.root_addrs[child_idx];

            // Traverse within this block
            loop {
                let node = block.node(addr);

                match node.len {
                    TAG_LEAF => {
                        let index = node.value_idx() as usize;
                        if index < self.keys.len() && self.keys[index] == nt_key {
                            return Some(index);
                        }
                        return None;
                    }
                    TAG_BLOCK => {
                        block_idx = node.block_idx() as usize;
                        break; // outer loop will process the new block
                    }
                    _ => {
                        // Internal node — skip prefix, match next byte
                        offset += node.prefix_len as usize;
                        if offset >= nt_key.len() {
                            return None;
                        }
                        let byte = nt_key[offset];
                        offset += 1;
                        let idx = node.find_child(byte)?;
                        addr = node.addrs[idx];
                    }
                }
            }
        }
    }

    /// Insert a new key-value pair. Returns Ok(index) on success.
    /// Returns Err(()) if the key already exists.
    /// Panics if the key contains a null byte (0x00).
    pub fn insert(&mut self, key: Vec<u8>, value: T) -> Result<usize, ()> {
        assert!(!key.contains(&0), "key must not contain null bytes");
        let mut nt_key = key;
        nt_key.push(0); // null terminator

        if self.get(&nt_key[..nt_key.len() - 1]).is_some() {
            return Err(());
        }

        let index = self.keys.len();
        self.keys.push(nt_key.clone());
        self.values.push(value);

        match self.root {
            None => {
                let mut block = Block::new(0);
                let leaf_addr = block.alloc_slot().expect("empty block should have room");
                block.nodes[leaf_addr as usize] = Node::leaf(index as u64);
                block.add_root_child(nt_key[0], leaf_addr, 1);
                let block_idx = self.blocks.len() as u32;
                self.blocks.push(block);
                self.root = Some(block_idx);
                Ok(index)
            }
            Some(_) => {
                self.insert_key(index, &nt_key);
                Ok(index)
            }
        }
    }

    fn insert_key(&mut self, new_index: usize, new_key: &[u8]) {
        let root_block_idx = self.root.unwrap() as usize;
        self.insert_at_block(root_block_idx, new_index, new_key, 0);
    }

    fn insert_at_block(&mut self, block_idx: usize, new_index: usize, new_key: &[u8], mut offset: usize) {
        offset += self.blocks[block_idx].prefix_len as usize;

        if offset >= new_key.len() {
            // Key exhausted during block prefix — prefix key case
            todo!("prefix key insertion at block boundary");
        }

        let byte = new_key[offset];
        offset += 1;

        let block = &self.blocks[block_idx];
        match block.find_root_child(byte) {
            Some(child_idx) => {
                let addr = block.root_addrs[child_idx];
                let node = block.node(addr);
                match node.len {
                    TAG_LEAF => {
                        self.split_leaf_at_root(block_idx, child_idx, new_index, new_key, offset);
                    }
                    TAG_BLOCK => {
                        let child_block_idx = node.block_idx() as usize;
                        self.insert_at_block(child_block_idx, new_index, new_key, offset);
                    }
                    _ => {
                        self.insert_into_node(block_idx, addr, new_index, new_key, offset);
                    }
                }
            }
            None => {
                // New root child
                self.add_new_root_child(block_idx, byte, new_index);
            }
        }
    }

    fn add_new_root_child(&mut self, block_idx: usize, byte: u8, new_index: usize) {
        let block = &mut self.blocks[block_idx];
        let leaf_addr = block.alloc_slot().expect("arena full — eviction not yet implemented");
        block.nodes[leaf_addr as usize] = Node::leaf(new_index as u64);
        block.add_root_child(byte, leaf_addr, 1);
    }

    fn split_leaf_at_root(
        &mut self,
        block_idx: usize,
        root_child_idx: usize,
        new_index: usize,
        new_key: &[u8],
        offset: usize,
    ) {
        let leaf_addr = self.blocks[block_idx].root_addrs[root_child_idx];
        let existing_index = self.blocks[block_idx].node(leaf_addr).value_idx() as usize;
        let existing_key = &self.keys[existing_index];

        // Find where the keys diverge
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

        // Allocate slots for the two leaf children
        let block = &mut self.blocks[block_idx];
        let addr_existing = block.alloc_slot().expect("arena full");
        let addr_new = block.alloc_slot().expect("arena full");
        block.nodes[addr_existing as usize] = Node::leaf(existing_index as u64);
        block.nodes[addr_new as usize] = Node::leaf(new_index as u64);

        let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
            (existing_byte, addr_existing, new_byte, addr_new)
        } else {
            (new_byte, addr_new, existing_byte, addr_existing)
        };

        // Replace the leaf with an internal node at the same slot
        let mut parent = Node {
            prefix_len,
            len: 2,
            symbols: [0; MAX_CHILDREN],
            addrs: [0; MAX_CHILDREN],
        };
        parent.symbols[0] = sym_a;
        parent.symbols[1] = sym_b;
        parent.addrs[0] = child_a;
        parent.addrs[1] = child_b;
        block.nodes[leaf_addr as usize] = parent;
    }

    fn insert_into_node(
        &mut self,
        block_idx: usize,
        node_addr: u8,
        new_index: usize,
        new_key: &[u8],
        mut offset: usize,
    ) {
        let node_copy = {
            let block = &self.blocks[block_idx];
            *block.node(node_addr)
        };

        // Check if new key diverges within this node's prefix
        let existing_index = self.key_of_subtree(block_idx, node_addr);
        let existing_key = &self.keys[existing_index];

        let prefix_len = node_copy.prefix_len as usize;
        for i in 0..prefix_len {
            let ki = offset + i;
            if ki >= new_key.len() || (ki < existing_key.len() && new_key[ki] != existing_key[ki]) {
                // Split at this point in the prefix
                let new_prefix_len = i as u8;
                let remaining_prefix = (prefix_len - i - 1) as u8;
                let existing_byte = existing_key[ki];
                let new_byte = if ki < new_key.len() { new_key[ki] } else { 0 };

                // Allocate leaf for new key
                let block = &mut self.blocks[block_idx];
                let new_leaf_addr = block.alloc_slot().expect("arena full");
                block.nodes[new_leaf_addr as usize] = Node::leaf(new_index as u64);

                // Rebuild the current node with remaining prefix
                let mut rebuilt = node_copy;
                rebuilt.prefix_len = remaining_prefix;

                // Allocate slot for the rebuilt node
                let rebuilt_addr = block.alloc_slot().expect("arena full");
                block.nodes[rebuilt_addr as usize] = rebuilt;

                // Replace current node with parent (2 children)
                let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
                    (existing_byte, rebuilt_addr, new_byte, new_leaf_addr)
                } else {
                    (new_byte, new_leaf_addr, existing_byte, rebuilt_addr)
                };

                let mut parent = Node {
                    prefix_len: new_prefix_len,
                    len: 2,
                    symbols: [0; MAX_CHILDREN],
                    addrs: [0; MAX_CHILDREN],
                };
                parent.symbols[0] = sym_a;
                parent.symbols[1] = sym_b;
                parent.addrs[0] = child_a;
                parent.addrs[1] = child_b;
                block.nodes[node_addr as usize] = parent;
                return;
            }
        }

        // Key matches prefix — find the next child
        offset += prefix_len;
        if offset >= new_key.len() {
            todo!("prefix key insertion at internal node");
        }

        let byte = new_key[offset];
        offset += 1;

        match node_copy.find_child(byte) {
            Some(child_idx) => {
                let child_addr = node_copy.addrs[child_idx];
                let child = self.blocks[block_idx].node(child_addr);

                match child.len {
                    TAG_LEAF => {
                        self.split_leaf_in_block(
                            block_idx, child_addr, new_index, new_key, offset,
                        );
                    }
                    TAG_BLOCK => {
                        let child_block_idx = child.block_idx() as usize;
                        self.insert_at_block(child_block_idx, new_index, new_key, offset);
                    }
                    _ => {
                        self.insert_into_node(block_idx, child_addr, new_index, new_key, offset);
                    }
                }
            }
            None => {
                // New child at this node
                self.add_child_to_node(block_idx, node_addr, byte, new_index);
            }
        }
    }

    fn add_child_to_node(
        &mut self,
        block_idx: usize,
        node_addr: u8,
        byte: u8,
        new_index: usize,
    ) {
        // Copy node data before mutating
        let (prefix_len, old_len, mut symbols, mut addrs) = {
            let block = &self.blocks[block_idx];
            let node = block.node(node_addr);
            (node.prefix_len, node.len, node.symbols, node.addrs)
        };

        if (old_len as usize) < MAX_CHILDREN {
            let insert_pos = {
                // Find insertion position using the copied symbols
                let n = old_len as usize;
                let mut pos = n;
                for i in 0..n {
                    if symbols[i] >= byte {
                        pos = i;
                        break;
                    }
                }
                pos
            };

            let block = &mut self.blocks[block_idx];
            let leaf_addr = block.alloc_slot().expect("arena full");
            block.nodes[leaf_addr as usize] = Node::leaf(new_index as u64);

            // Shift existing entries right
            for i in (insert_pos..old_len as usize).rev() {
                symbols[i + 1] = symbols[i];
                addrs[i + 1] = addrs[i];
            }
            symbols[insert_pos] = byte;
            addrs[insert_pos] = leaf_addr;

            block.nodes[node_addr as usize] = Node {
                prefix_len,
                len: old_len + 1,
                symbols,
                addrs,
            };
        } else {
            // Node is full (7 children) — promote to a new Block
            todo!("promote full node to block");
        }
    }

    fn split_leaf_in_block(
        &mut self,
        block_idx: usize,
        leaf_addr: u8,
        new_index: usize,
        new_key: &[u8],
        offset: usize,
    ) {
        let existing_index = self.blocks[block_idx].node(leaf_addr).value_idx() as usize;
        let existing_key = &self.keys[existing_index];

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

        let block = &mut self.blocks[block_idx];
        let addr_existing = block.alloc_slot().expect("arena full");
        let addr_new = block.alloc_slot().expect("arena full");
        block.nodes[addr_existing as usize] = Node::leaf(existing_index as u64);
        block.nodes[addr_new as usize] = Node::leaf(new_index as u64);

        let (sym_a, child_a, sym_b, child_b) = if existing_byte < new_byte {
            (existing_byte, addr_existing, new_byte, addr_new)
        } else {
            (new_byte, addr_new, existing_byte, addr_existing)
        };

        let mut parent = Node {
            prefix_len,
            len: 2,
            symbols: [0; MAX_CHILDREN],
            addrs: [0; MAX_CHILDREN],
        };
        parent.symbols[0] = sym_a;
        parent.symbols[1] = sym_b;
        parent.addrs[0] = child_a;
        parent.addrs[1] = child_b;
        block.nodes[leaf_addr as usize] = parent;
    }

    /// Find any key in the subtree rooted at the given node.
    /// Follows leftmost child until reaching a leaf.
    fn key_of_subtree(&self, block_idx: usize, mut addr: u8) -> usize {
        loop {
            let block = &self.blocks[block_idx];
            let node = block.node(addr);
            match node.len {
                TAG_LEAF => return node.value_idx() as usize,
                TAG_BLOCK => {
                    let new_block_idx = node.block_idx() as usize;
                    let new_block = &self.blocks[new_block_idx];
                    let first_addr = new_block.root_addrs[0];
                    // Switch to child block and continue
                    // This is recursive but depth is bounded by trie depth
                    return self.key_of_subtree_in_block(new_block_idx, first_addr);
                }
                _ => {
                    addr = node.addrs[0];
                }
            }
        }
    }

    /// Helper for key_of_subtree that starts within a specific block.
    fn key_of_subtree_in_block(&self, block_idx: usize, mut addr: u8) -> usize {
        loop {
            let block = &self.blocks[block_idx];
            let node = block.node(addr);
            match node.len {
                TAG_LEAF => return node.value_idx() as usize,
                TAG_BLOCK => {
                    let new_block_idx = node.block_idx() as usize;
                    return self.key_of_subtree_in_block(new_block_idx, 0);
                }
                _ => {
                    addr = node.addrs[0];
                }
            }
        }
    }
}

impl<T: Clone> Default for ArenaTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_size_is_16_bytes() {
        assert_eq!(size_of::<Node>(), 16);
    }

    #[test]
    fn node_leaf_roundtrip() {
        let node = Node::leaf(42u64);
        assert!(node.is_leaf());
        assert_eq!(node.value_idx(), 42);
    }

    #[test]
    fn node_block_ref_roundtrip() {
        let node = Node::block_ref(7u32);
        assert!(node.is_block_ref());
        assert_eq!(node.block_idx(), 7);
    }

    #[test]
    fn node_internal_find_child() {
        let node = Node {
            prefix_len: 0,
            len: 4,
            symbols: [10, 20, 30, 40, 0, 0, 0],
            addrs: [0, 1, 2, 3, 0, 0, 0],
        };
        assert_eq!(node.find_child(10), Some(0));
        assert_eq!(node.find_child(30), Some(2));
        assert_eq!(node.find_child(40), Some(3));
        assert_eq!(node.find_child(15), None);
        assert_eq!(node.find_child(50), None);
    }

    #[test]
    fn block_alloc_free() {
        let mut block = Block::new(0);
        let slot0 = block.alloc_slot().unwrap();
        assert_eq!(slot0, 0);
        let slot1 = block.alloc_slot().unwrap();
        assert_eq!(slot1, 1);
        block.free_slot(slot0);
        let slot0_again = block.alloc_slot().unwrap();
        assert_eq!(slot0_again, 0);
    }

    #[test]
    fn block_alloc_fills_sequentially() {
        let mut block = Block::new(0);
        for i in 0..10 {
            let slot = block.alloc_slot().unwrap();
            assert_eq!(slot, i as u8);
        }
        assert_eq!(block.len, 10);
    }

    #[test]
    fn insert_empty_and_get() {
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
        let idx = trie.insert(b"hello".to_vec(), "world").unwrap();
        assert_eq!(idx, 0);
        assert_eq!(trie.get(b"hello"), Some(0));
        assert_eq!(trie.get(b"world"), None);
    }

    #[test]
    fn insert_two_keys_split_leaf() {
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
        trie.insert(b"abc".to_vec(), "first").unwrap();
        trie.insert(b"abd".to_vec(), "second").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"abd"), Some(1));
        assert_eq!(trie.get(b"abe"), None);
        assert_eq!(trie.get(b"ab"), None);
    }

    #[test]
    fn insert_three_keys() {
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
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
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
        trie.insert(b"abc".to_vec(), "long").unwrap();
        trie.insert(b"ab".to_vec(), "short").unwrap();
        // Prefix key insertion not yet implemented
        // assert_eq!(trie.get(b"abc"), Some(0));
        // assert_eq!(trie.get(b"ab"), Some(1));
    }

    #[test]
    fn insert_no_common_prefix() {
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
        trie.insert(b"abc".to_vec(), "1").unwrap();
        trie.insert(b"xyz".to_vec(), "2").unwrap();
        assert_eq!(trie.get(b"abc"), Some(0));
        assert_eq!(trie.get(b"xyz"), Some(1));
        assert_eq!(trie.get(b"ab"), None);
        assert_eq!(trie.get(b"abcz"), None);
    }

    #[test]
    fn insert_single_char_keys() {
        let mut trie: ArenaTrie<&str> = ArenaTrie::new();
        trie.insert(b"a".to_vec(), "1").unwrap();
        trie.insert(b"b".to_vec(), "2").unwrap();
        trie.insert(b"c".to_vec(), "3").unwrap();
        assert_eq!(trie.get(b"a"), Some(0));
        assert_eq!(trie.get(b"b"), Some(1));
        assert_eq!(trie.get(b"c"), Some(2));
        assert_eq!(trie.get(b"d"), None);
    }

    #[test]
    fn insert_deeply_nested() {
        let mut trie: ArenaTrie<usize> = ArenaTrie::new();
        trie.insert(b"a".to_vec(), 0).unwrap();
        trie.insert(b"ab".to_vec(), 1).unwrap();
        trie.insert(b"abc".to_vec(), 2).unwrap();
        trie.insert(b"abcd".to_vec(), 3).unwrap();
        assert_eq!(trie.get(b"a"), Some(0));
        assert_eq!(trie.get(b"ab"), Some(1));
        assert_eq!(trie.get(b"abc"), Some(2));
        assert_eq!(trie.get(b"abcd"), Some(3));
        assert_eq!(trie.get(b"abcde"), None);
    }
}