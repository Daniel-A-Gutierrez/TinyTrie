# Project TODO

## NibbleTrie

1. **remove()** — Currently impossible. Needs leaf removal, node collapse (when a node drops to 1 child), and upward propagation of `leaf` field updates. Also need to clear `terminal` when removing a prefix key.

2. **Configurable index width** — `const INDEX: usize = 4` (u32) vs 2 (u16) would halve the node size to ~40 bytes for small tries. Needs `arena: Vec<Node<INDEX>>` and `children: [u16; 16]` in the u16 variant.

3. **Drop the dummy key** — `index[0] = (0, 0)` dummy costs an entry and makes `get_value` do `-1` offset math. Use `+1` encoding consistently for `children[]` leaf slots instead.

4. **Iterator sentinel cleanup** — Replace `usize::MAX` sentinel with a bool flag or separate "initialized" state.

5. **seek() fallback** — When leaf key < seek key, calling `self.next()` rebuilds the stack. Could advance within the current node instead.

## PolyTrie

6. **Parallel occupancy arrays** — `occ16: Vec<u16>` and `occ256: Vec<[u8; 32]>` maintained on insert/graduate. Eliminates `compute_mask()` on push and O(256) linear scans for Node256.

7. **SIMD compute_mask for Node16** — Load 16 discriminant bytes at stride-8 offsets, compute mask in ~5 instructions.

8. **Inline digit_at** — Specialize hot-path cases for 2-bit and 8-bit radix (most common after graduation). Avoid `match n` dispatch.

9. **Key string table** — Replace `keys: Vec<Vec<u8>>` with contiguous buffer + side index (like NibbleTrie's `buf` + `index`). Saves ~24 bytes/key.

10. **u16 index variant** — For small tries, u16 arena indices would halve node sizes.

## BitTrie

11. **Terminal flag** — Like NibbleTrie, allow `0x00` bytes in keys and use a `terminal` flag instead of null terminators. Would require reworking `get()`, `insert()`, and `seek()`.

12. **Flat buffer key storage** — Same as NibbleTrie's `buf` + `index` approach, replacing `keys: Vec<Vec<u8>>`.

13. **optimize()** — BFS reorder for cache locality (like NibbleTrie and PolyTrie have).

14. **Configurable index width** — u16 variant for small tries.

## TinyTrie (prefix_trie.rs)

15. **Flat buffer key storage** — Replace `keys: Vec<Vec<u8>>` with contiguous buffer.

16. **In-place INode child mutation** — Currently builds a new Vec on every descent into an INode child. Could do in-place `ptr::write`.

17. **Fold duplicate check into insert** — Currently does a full `self.get()` before insert. Could detect duplicates at Leaf in `insert_into_node`.

## Cross-cutting

18. **Serialization** — Write arenas as contiguous blobs, mmap-friendly. All three arena-based tries (NibbleTrie, BitTrie, PolyTrie) could share a serialization format.

19. **Bench harness** — The `TinyTrieMap` trait is implemented for all four tries. Bench results in `benches/bench_results.md`.