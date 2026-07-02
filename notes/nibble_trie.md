# Nibble Trie

A fixed-fanout radix trie indexed by nibbles (4-bit half-bytes). Each internal node has 16 child slots addressed by direct array index — no binary search, no branch misprediction on the child path. A `prefix_len` field compresses shared nibble sequences into a single node, and a `leaf` field gives O(1) access to a reference key during insertion.

## Structure

```
Node<PTR, LEN> (default PTR=u32, LEN=u16 → 76 bytes; PTR=u16, LEN=u16 → 40 bytes):
    children:   [OptNz<PTR>; 16]  // 0 (None-ish) = empty; leaf key index or arena index otherwise
    prefix_len:  LEN              // absolute nibble position of the discriminating nibble
    leaf_mask:   u16              // bit N set → children[N] is a leaf key index
    leaf:        OptNz<PTR>       // key index of a reference/descendant leaf (for retrieval)
    terminal:    bool             // true → this node's own key ends here (prefix key)
```

`OptNz<PTR>` is a `#[repr(transparent)]` newtype over `PTR` where the value `0` means
"empty" and any nonzero value is a real index — a stable, no-`unsafe`-on-access equivalent
of `Option<NonZero<PTR>>`. `[OptNz<PTR>; 16]` is layout-identical to `[PTR; 16]`, so the
SIMD `children_mask` is reused via a single `repr(transparent)` pointer cast. Real arena
child addresses are `>= 1` (root is arena[0] and never a child target) and real key indices
are `>= 1` (index[0] is a dummy entry), so `0` is free as the sentinel.

```
NibbleTrie<K, T, PTR=u32, LEN=u16>  (K: ByteKey):
    arena:  Vec<Node<PTR, LEN>>   // arena[0] = root; PTR indices replace pointer trees
    buf:    Vec<u8>                // all keys concatenated (no null terminators)
    index:  Vec<(usize, LEN)>      // (offset into buf, len) per key — offset is usize, len is compact
    values: Vec<T>                 // values[i] ↔ index[i+1]
```

> **Node stacking (`STAK`) was reverted.** An experiment (commit `34b612e`) let multiple
> "virtual nodes" share one physical `Node` via a `STAK` const generic and per-vnode
> `occupancy`/`leaf_mask`/`prefix_len` arrays, with `optimize()` packing ancestor-descendant
> vnodes together. It was abandoned — `nibble_trie.rs` is back to one logical node per
> physical node, using `OptNz<PTR>` (`0` sentinel) instead of the `PTR::max_value` sentinel
> that stacking required. The `TrieIndex` trait retains `max_value_sentinel()` because the
> sibling tries (`fixed_len_nibble_trie`, `nib_trie`) still use it.

## TrieIndex Trait

`PTR` and `LEN` must implement `TrieIndex` — provides `as_usize()`, `max_value()`, `zero()`, `from_usize()`, `children_mask()` (and `max_value_sentinel()`, kept for the sibling tries). Implemented for `u8`, `u16`, `u32`, `u64`.

- `children_mask` dispatches by type: `u16` → `simd::children_mask_u16` (Simd<u16,16>), `u32` → `simd::children_mask` (u32x16), `u64` → `simd::children_mask_u64` (two Simd<u64,8> lanes). `nibble_trie`'s `Node::children_mask()` casts its `[OptNz<PTR>; 16]` to `[PTR; 16]` (layout-identical) and calls this.

## Node Sizes

| PTR   | LEN | children | prefix_len | leaf_mask | leaf | terminal | total |
|-------|-----|----------|-----------|-----------|------|----------|-------|
| u32   | u16 | 64       | 2         | 2         | 4    | 1 (+3 pad) | 76  |
| u16   | u16 | 32       | 2         | 2         | 2    | 1 (+1 pad) | 40  |
| u8    | u16 | 16       | 2         | 2         | 1    | 1          | 22  |

With `PTR=u16`, node drops to 40 bytes — fits a 64-byte cache line with room to spare.

## Terminal flag

`Node.terminal: bool` — set when a key ends exactly at this node (a prefix of some
descendant key, e.g. "ab" in {"ab", "abc"}). Replaces the older `offset: u64` (terminal in
bit 63) scheme; `leaf`/`leaf_mask` now carry the reference-key info that `offset` used to
provide, so there is no separate raw-offset field.

## Key Design Decisions

**Absolute `prefix_len`** — Each node stores the absolute nibble position of its discriminating nibble, not a relative offset. During lookup, `key_nibble_at(key, node.prefix_len)` directly selects the child — no accumulation across levels. This also simplifies insertion: `find_divergence()` takes a `from` parameter that skips already-confirmed-matching nibbles.

**Terminal flag (no null terminators)** — Keys that are prefixes of other keys (e.g. "ab" in {"ab", "abc"}) are represented by the `terminal: bool` flag on the node where the key ends, rather than a null-byte leaf child. This eliminates null terminators, allows `0x00` bytes in keys, and makes `get()` accept plain `&[u8]`.

**Flat buffer key storage** — Keys are stored contiguously in `buf: Vec<u8>` with a side index `Vec<(usize, LEN)>` of (offset, len) pairs. A dummy entry at `index[0] = (0, LEN::zero())` points at `buf[0]` (unused byte). Real keys start at index 1. This saves ~24 bytes/key vs `Vec<Vec<u8>>` (one heap allocation per key). The `0` sentinel in `children[]` and `leaf` (via `OptNz`) still works because `index[0]` is the dummy, so real key indices are `>= 1`.

**`leaf` field** — Every node carries a `leaf: OptNz<PTR>` key index (for retrieval — value lookup via `values[leaf-1]`, key via `index[leaf]`) and as the reference key for insertion divergence comparison. When terminal, `leaf` is the node's own key index. When not terminal, it points to a descendant leaf key index. Set at node creation: `leaf = key_index`.

**Empty-slot encoding (`OptNz`)** — Child slots and `leaf` use `OptNz<PTR>` (`#[repr(transparent)]` over `PTR`, `0` = empty) instead of a `PTR::max_value` sentinel. `[OptNz<PTR>; 16]` is layout-identical to `[PTR; 16]`, so `Node::children_mask()` casts to the raw array and reuses the SIMD `children_mask`. No tag byte, no `unsafe` on field access.

**Arena allocation** — All nodes live in a `Vec<Node<PTR, LEN>>` with `PTR` indices. No `Box`, no `unsafe`, no manual `free_subtree`. The root is always `arena[0]`. Drop is automatic. Arena indices start at 1 for children (since 0 = root and no child points to the root).

**Insert-time overflow checks** — `insert()` checks `arena.len() >= PTR::max_value()` and `index.len() >= PTR::max_value()` (arena child addresses and key indices must be nonzero and fit in `PTR`), plus `key.len() * 2 > LEN::max_value()` (nibble count exceeds LEN capacity). Returns `Err(())` on overflow.

**`children_mask()` via SIMD** — `Node::children_mask()` dispatches to `PTR::children_mask()` which uses SIMD (u16x16, u32x16, or two u64x8 lanes) to compute a 16-bit occupancy mask. Used by the iterator for O(1) sibling navigation via `trailing_zeros`/`leading_zeros`.

**BFS-maintained insertion order** — `sort_internal_children()` is called after every insertion that adds a new arena node. It ensures lower nibble positions point to lower arena addresses, maintaining a breadth-first-like layout without requiring an explicit `optimize()` call. Uses `swap_arena()` to rotate nodes into position.

## SIMD Divergence Scan

`simd_find_divergence()` replaces the nibble-by-nibble scalar scan for key comparison during insertion. It compares N bytes at a time using `Simd::<u8, N>`, finds the first differing byte via `simd_ne` + `first_set`, then resolves the diverging nibble with a branchless XOR trick (`diverging_nibble`). A scalar `find_divergence()` call handles the tail. The `from` parameter lets the caller skip already-confirmed-matching prefix bytes, halving comparison work in deep tries.

## Lookup

```
node = arena[0]
loop:
    if node.prefix_len >= max_nib:
        if node.is_terminal(): verify key == index[node.leaf], return index
        return None
    nib = key_nibble_at(key, node.prefix_len)
    slot = node.children[nib]
    if slot.is_none(): return None
    if node.is_leaf(nib): verify key == key_slice(slot.get()), return index or None
    node = arena[slot.get()]
```

No prefix verification during descent — the structure is trusted (known-in-set assumption). The final key comparison at the leaf catches any mismatch. Terminal nodes are checked when key nibbles are exhausted.

## Insertion

At each node, `find_divergence(new_key, ref_key, confirmed)` scans from the `confirmed` position onward, skipping nibbles already known to match from prior descent levels. Three cases:

1. **Duplicate** — Keys are identical. Return `Err(())`.

2. **Divergence before `prefix_len`** — Split the node. Create a new parent at the divergence position; the old node becomes a child. If the new key ends at the split point, the new parent gets `terminal = true`.

3. **Divergence at or after `prefix_len`** — Follow the child selected by `key_nibble_at(key, prefix_len)`. If key nibbles are exhausted at this node, mark `terminal = true`. If the slot is empty, insert a leaf. If it's a leaf, split it into a new internal node with two children at the divergence position. If one key ends at the divergence point, the split node gets `terminal = true`.

**`confirmed` tracking** — A `usize` variable starts at 0 and advances to `prefix_len + 1` each time we descend through a node. Starting the divergence scan from `confirmed` avoids re-scanning the shared prefix at every level — roughly halving comparison work in deep tries.

**`DivergeResult`** — Returns `Duplicate` (identical keys) or `At(pos)` (divergence position, or end-of-shorter-key for prefix keys).

## Iterator

Stack-based `(arena_index: PTR, children_mask: u16, nibble_position: usize)` triples. The `children_mask` is computed once when a node is pushed onto the stack, then used for all sibling transitions:

- **Forward**: `mask >> (nib + 1)` → `trailing_zeros` finds next sibling in O(1)
- **Backward**: `mask & ((1 << nib) - 1)` → `leading_zeros` finds previous sibling in O(1)
- **Descent to first/last leaf**: `mask.trailing_zeros()` / `(15 - mask.leading_zeros())` for O(1) first/last child

Three nibble-position states:
- `usize::MAX` — before first child (initial state)
- `0..16` — positioned at child slot (may be leaf or internal)
- `TERMINAL_NIB (16)` — positioned at the node's terminal value

`seek()` follows key nibbles through the trie. When the exact-match leaf key is less than the seek key, it advances past it and backtracks through ancestor stack to find the next valid position.

**Index-only methods** — `next_index()` and `prev_index()` return just the key index, skipping the key buffer and value reads. Useful for random-access cursor patterns where key/value reads hit scattered offsets and defeat prefetching.

## Optimize (DFS key-sorted buf rewrite)

`optimize()` rewrites `buf` so keys appear in sorted (DFS) order with contiguous layout, and reorders `index`/`values` to match:

1. Pre-allocates `new_buf` at exact size (`vec![0u8; buf_len]`).
2. DFS walk copies keys via `copy_from_slice` (no per-key `extend_from_slice` overhead), recording the visitation order in `dfs_key_order`.
3. Updates `index[].0` offsets in place during the walk.
4. Builds a `key_remap` (old key index → new 1-based DFS rank), then remaps the key indices stored in each node's `leaf` and leaf-children slots.
5. Rebuilds `index` in DFS order and reorders `values` via `ptr::read` + `set_len(0)` swap (no `T: Clone` required).
6. Truncates `new_buf` to the live cursor.

The arena topology (child structure) is unchanged — only key indices are remapped, no arena rebuild or address remapping. After `optimize()`, a forward iteration hits `buf` in ascending memory order — sequential, prefetcher-friendly access. Idempotent — a second call with no intervening inserts rewrites the same order.

## Benchmarks

See `benches/bench_results.md` for current numbers.

## Todo

High Impact

1. **remove()** — Currently impossible. Needs to handle: leaf removal, node collapse when a node drops to 1 child, and updating `leaf` fields on ancestors. The `leaf` field invariant ("always points to a descendant") breaks on removal unless propagated upward. Also need to clear `terminal` when removing a prefix key.

Medium Impact

2. **Drop the dummy key** — The `index[0] = (0, LEN::zero())` dummy costs an allocation and makes `get_value` do `-1` offset math. Instead, use `+1` encoding consistently for `children[]` leaf slots. The `leaf` field can still use direct indices since it's always set on creation.

3. **Iterator sentinel cleanup** — Replace the `usize::MAX` sentinel with a bool flag on the stack or a separate "initialized" state. Cleaner and avoids the `if nib == usize::MAX` branching.

4. **seek() uses next() as fallback** — When the exact-match leaf key is less than the seek key, we call `self.next()` which rebuilds the stack. We could instead just advance within the current node.

5. **Reorder Node fields** — `children` is the first field (offset 0), which is good for cache locality since it's accessed first in lookup. Consider whether `prefix_len` and `leaf_mask` should be packed into a single u32 (they're both u16) to save 2 bytes of padding, though this doesn't affect the overall 76-byte size due to alignment.

Low Impact / Code Quality

6. **Debug visualization** — A Dot or Debug impl that renders the trie structure would make debugging much easier.