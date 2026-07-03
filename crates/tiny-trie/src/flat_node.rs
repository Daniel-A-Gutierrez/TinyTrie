//! FlatNode (Fnode) — a dense leaf-pack node for the nibble trie.
//!
//! **Phase 0 experiment file.** These types are defined standalone to measure
//! their `size_of`; they are NOT yet wired into `NibbleTrie`'s arena. See
//! `/home/d/.claude/plans/buzzing-giggling-hedgehog.md` (Phase 0).
//!
//! An Fnode collapses a small/deep subtree (≤ `CAP` keys) into one node holding a
//! flattened pre-order micro-trie: per slot a `prefix_len` (discriminating depth),
//! a nibble (packed in `nibbles`), and an `OptNz<PTR>` key-index ptr (`None` =
//! branch marker, `Some` = terminal key index into `index`). An Fnode is a DAG
//! leaf — slots hold only key indices, never arena refs.
//!
//! Three arena-element representations are measured:
//! - `ArenaNode` — a tagged `enum` (Inode | Fnode). Simplest; costs a tag byte and
//!   sizes every slot to `max(Inode, Fnode)`.
//! - `UntaggedArenaNode` — a `union` (no tag field); the Inode-vs-Fnode
//!   discriminator would live in the parent's child-PTR high bit. Sizes to
//!   `max(Inode, Fnode)` like the enum but saves the tag byte.

use std::mem::ManuallyDrop;

use crate::nibble_trie::{Node, OptNz, TrieIndex};
use crate::tiny_array::TinyArray;

/// A dense leaf-pack node: `nibbles` (4 bits × `CAP`) + a `TinyArray` of
/// `(key-index ptr, prefix_len)` slots. `len` lives inside `TinyArray`.
pub struct FlatNode<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    pub nibbles: u64,
    pub slots: TinyArray<(OptNz<PTR>, LEN), CAP>,
}

impl<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize> FlatNode<PTR, LEN, CAP>
where
    [(); CAP]:
{
    pub fn new() -> Self {
        FlatNode {
            nibbles: 0,
            slots: TinyArray::new(),
        }
    }
}

impl<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize> Default for FlatNode<PTR, LEN, CAP>
where
    [(); CAP]:
{
    fn default() -> Self {
        Self::new()
    }
}

/// Tagged arena element: `Inode` (the existing 16-slot direct-addressed `Node`)
/// or `Fnode` (a [`FlatNode`]). Not `Copy` (Fnode has `Drop` via `TinyArray`).
pub enum ArenaNode<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    Inode(Node<PTR, LEN>),
    Fnode(FlatNode<PTR, LEN, CAP>),
}

/// Untagged arena element: a `union` of `Inode` | `Fnode` with no tag field. The
/// Inode-vs-Fnode discriminator is intended to live in the parent's child-PTR
/// high bit (not measured here). `Fnode` is wrapped in `ManuallyDrop` because a
/// `union` field must be `Copy` and `FlatNode` is not (it owns `TinyArray`).
#[repr(C)]
pub union UntaggedArenaNode<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    pub inode: Node<PTR, LEN>,
    pub fnode: ManuallyDrop<FlatNode<PTR, LEN, CAP>>,
}

// --- u32 nibble-pack variants (CAP <= 8) -------------------------------------
//
// For CAP <= 8 the nibble pack fits in 32 bits, so `nibbles: u32` drops Fnode to
// 4-byte alignment and lets it fit inside the Inode footprint (76 B for u32/u32,
// 40 B for u16/u16) — an untagged union then adds NO Inode bloat.

/// Variant of [`FlatNode`] with a `u32` nibble pack (holds up to 8 nibbles).
pub struct FlatNodeN32<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    pub nibbles: u32,
    pub slots: TinyArray<(OptNz<PTR>, LEN), CAP>,
}

/// Tagged-enum variant paired with [`FlatNodeN32`].
pub enum ArenaNodeN32<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    Inode(Node<PTR, LEN>),
    Fnode(FlatNodeN32<PTR, LEN, CAP>),
}

/// Untagged-union variant paired with [`FlatNodeN32`].
#[repr(C)]
pub union UntaggedArenaNodeN32<PTR: TrieIndex, LEN: TrieIndex, const CAP: usize>
where
    [(); CAP]:
{
    pub inode: Node<PTR, LEN>,
    pub fnode: ManuallyDrop<FlatNodeN32<PTR, LEN, CAP>>,
}