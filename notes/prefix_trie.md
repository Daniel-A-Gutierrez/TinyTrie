# TinyTrie — Prefix-Compressed Radix Trie

## What It Is

A prefix-compressed radix trie (DFA) with existence guarantee. Keys are null-terminated (`0x00` is just another byte, so "ab" is `[0x61, 0x62, 0x00]`). Every node is exactly 16 bytes (for the default `INLINE=6, PREFIX=u8` config). Tag encoding: 0=HNode, 1=Leaf, 2+=INode (tag=child count).

**Status**: Original implementation. Still uses null-terminator contract and `Vec<Vec<u8>>` key storage. Has not been migrated to terminal flag or flat buffer.

## Architecture

- **Tag encoding**: `0` = HNode (heap-allocated children, 7+), `1` = Leaf (terminal), `2..=INLINE` = INode (inline children, tag = count), `>INLINE` = PairVec
- **Node size**: 16 bytes (with INLINE=6, PREFIX=u8). All bytes including padding are zero-initialized for Miri-safe SIMD.
- **Key storage**: `keys: Vec<Vec<u8>>` with null terminators. No flat buffer or side index.
- **Null-terminated keys**: The `\0` byte is just another symbol in the DFA. Shorter keys naturally sort before extensions.
- **All types are Copy**: `Trie` derives Copy. ManuallyDrop was reverted (caused 25-45% lookup regression via `&raw const` + `.cast()` overhead).

## Key Abstractions (implemented)

1. **`InternalNodeRef`** — read-only enum over `&INode` / `&PairVec`. Used in cold paths (free_subtree, leftmost_leaf, rightmost_leaf, seek, first_key).
2. **`InternalNodeOwned`** — owned enum for mutation. Methods: `from_box()`, `replace_child()` (in-place, O(1) for both INode and PairVec), `add_child()`, `split_prefix()`, `first_key()`, `into_trie()`.
3. **`PrefixResult` + `find_prefix_divergence`** — extracted prefix comparison loop, eliminates 3× duplication.
4. **`order_pair`** — eliminates repeated `if byte_a < byte_b` pattern.
5. **Fold duplicate check into insert** — `insert_into_node` returns `Result<_, ()>`, detects duplicates at Leaf via `PrefixResult::Matched`. No more `self.get()` before insert.
6. **SIMD count==0 guards** — early returns in all 4 SIMD functions.
7. **`free_pairvec_data` takes `&PairVec`** — no more raw pointer + capacity footgun.
8. **Inline methods on `Trie`** — `tag()`, `prefix_len()`, `find_child()`, `find_child_lower_bound()`, `children()`, `symbols()` as `#[inline(always)]` for hot paths.
9. **`get()` uses direct 3-branch match** — no `InternalNodeRef` dispatch, raw `unsafe { node.inode/pairvec }` field access.

## Performance Lesson

**`InternalNodeRef` is fine for cold paths** (free_subtree, iterators, first_key) but **kills hot paths** (`get()`). Each method call on the enum dispatches on the variant, and the compiler can't eliminate this even with inlining. Hot paths must use direct `match tag { }` with raw field access.

`ManuallyDrop` added ~2-3x overhead per union field access in tight loops. Direct `unsafe { node.inode }` (Copy-based) compiles to a simple memory load.

## TinyTrieMap Trait

All four tries (TinyTrie, NibbleTrie, BitTrie, PolyTrie) implement `TinyTrieMap`:
```rust
pub trait TinyTrieMap: Sized {
    fn trie_new() -> Self;
    fn trie_insert(&mut self, key: Vec<u8>, value: usize);
    fn trie_get(&self, key: &[u8]) -> Option<usize>;
    fn trie_iter_fwd(&self, f: impl FnMut(&[u8], &usize));
    fn trie_iter_rev(&self, f: impl FnMut(&[u8], &usize));
    fn trie_len(&self) -> usize;
    fn trie_optimize(&mut self) { /* default no-op */ }
}
```

This enables trait-based bench dispatch. NibbleTrie and PolyTrie implement `trie_optimize()`. TinyTrie and BitTrie use the default no-op.

## Open Issues

- **`PairVec::new()` uses `std::mem::zeroed()`** — padding might not be zeroed correctly for all INLINE values.
- **`promote_inode_to_pairvec` builds Vec unnecessarily** — initial size is fixed (INLINE+1).
- **`add_child_to_pairvec` returns by value** — could update ptr in-place instead.
- **No terminal flag** — still uses null terminators. Could adopt NibbleTrie's approach.
- **No flat buffer key storage** — still uses `Vec<Vec<u8>>`.
- **No `optimize()`** — no BFS reorder.