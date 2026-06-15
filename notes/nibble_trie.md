# Nibble Trie

A fixed-fanout radix trie indexed by nibbles (4-bit half-bytes). Each internal node has 16 child slots addressed by direct array index — no binary search, no branch misprediction on the child path. A `prefix_len` field compresses shared nibble sequences into a single node, and a `leaf` field gives O(1) access to a reference key during insertion.

## Structure

```
Node<PTR, LEN> (default PTR=u32, LEN=u16 → 80 bytes; PTR=u16, LEN=u16 → 48 bytes):
    children:   [PTR; 16]    // 0 = empty; leaf key index or arena index otherwise
    prefix_len:  LEN         // absolute nibble position of the discriminating nibble
    leaf_mask:   u16         // bit N set → children[N] is a leaf key index
    leaf:        PTR         // key index of a reference/descendant leaf (for retrieval)
    offset:      u64         // bit 63 = terminal flag, bits 0-62 = raw buf offset
```

```
NibbleTrie<T, PTR=u32, LEN=u16>:
    arena:  Vec<Node<PTR, LEN>>   // arena[0] = root; PTR indices replace pointer trees
    buf:    Vec<u8>                // all keys concatenated (no null terminators)
    index:  Vec<(usize, LEN)>      // (offset into buf, len) per key — offset is usize, len is compact
    values: Vec<T>                 // values[i] ↔ index[i]
```

## TrieIndex Trait

`PTR` and `LEN` must implement `TrieIndex` — provides `as_usize()`, `max_value()`, `zero()`, `from_usize()`, `children_mask()`. Implemented for `u16`, `u32`, `u64`.

- `children_mask` dispatches by type: `u16` → `simd::children_mask_u16` (Simd<u16,16>), `u32` → `simd::children_mask` (u32x16), `u64` → `simd::children_mask_u64` (two Simd<u64,8> lanes).

## Node Sizes

| PTR   | LEN | children | prefix_len | leaf_mask | leaf | offset | total |
|-------|-----|----------|-----------|-----------|------|--------|-------|
| u32   | u16 | 64       | 2         | 2         | 4    | 8      | 80    |
| u16   | u16 | 32       | 2         | 2         | 2    | 8      | 48    |

With `PTR=u16`, node drops from 80 → 48 bytes — fits a 64-byte cache line with room to spare.

## Packed Terminal in offset

`Node.offset: u64` — bit 63 stores the terminal flag, bits 0-62 store the raw buf offset. Accessors: `is_terminal()`, `set_terminal(bool)`, `raw_offset()`, `set_raw_offset(u64)`. Eliminates the old `terminal: bool` field (1 byte + 3 padding). For default PTR=u32/LEN=u16, this keeps Node at 80 bytes (u64 offset replaces u32 offset + u32 padding from the old `terminal` field).

## Key Design Decisions

**Absolute `prefix_len`** — Each node stores the absolute nibble position of its discriminating nibble, not a relative offset. During lookup, `key_nibble_at(key, node.prefix_len)` directly selects the child — no accumulation across levels. This also simplifies insertion: `find_divergence()` takes a `from` parameter that skips already-confirmed-matching nibbles.

**Terminal flag (no null terminators)** — Keys that are prefixes of other keys (e.g. "ab" in {"ab", "abc"}) are represented by the terminal bit in `node.offset`, rather than a null-byte leaf child. This eliminates null terminators, allows `0x00` bytes in keys, and makes `get()` accept plain `&[u8]`.

**Flat buffer key storage** — Keys are stored contiguously in `buf: Vec<u8>` with a side index `Vec<(usize, LEN)>` of (offset, len) pairs. A dummy entry at `index[0] = (0, LEN::zero())` points at `buf[0]` (unused byte). Real keys start at index 1. This saves ~24 bytes/key vs `Vec<Vec<u8>>` (one heap allocation per key). The `0` sentinel in `children[]` and `leaf` still works because `index[0]` is the dummy.

**`leaf` + `offset` fields** — Every node carries a `leaf: PTR` key index (for retrieval — value lookup via `values[leaf-1]`, key via `index[leaf]`) and an `offset: u64` direct buf offset with terminal bit packed in bit 63 (for insertion divergence comparison and `get()` terminal fast path). When terminal, `leaf` is the node's own key index and `offset` points directly to its key in `buf` — so `get()` on a terminal node avoids the `index` lookup entirely: `key = buf[raw_offset..raw_offset + prefix_len/2]`. When not terminal, `offset` points to a descendant key in `buf`. Set together at node creation: `leaf = key_index; offset = buf_offset | terminal_bit`.

**Arena allocation** — All nodes live in a `Vec<Node<PTR, LEN>>` with `PTR` indices. No `Box`, no `unsafe`, no manual `free_subtree`. The root is always `arena[0]`. Drop is automatic. Arena indices start at 1 for children (since 0 = root and no child points to the root).

**Insert-time overflow checks** — `insert()` checks `arena.len() >= PTR::max_value() / 2` (conservative: at least 1 node per leaf, often 2+ with splits), `key.len() * 2 > LEN::max_value()` (nibble count exceeds LEN capacity), and `buf.len() + key.len() > LEN::max_value()` (buf offset overflow). Returns `Err(())` on overflow.

**`children_mask()` via SIMD** — `Node::children_mask()` dispatches to `PTR::children_mask()` which uses SIMD (u16x16, u32x16, or two u64x8 lanes) to compute a 16-bit occupancy mask. Used by the iterator for O(1) sibling navigation via `trailing_zeros`/`leading_zeros`.

**BFS-maintained insertion order** — `sort_internal_children()` is called after every insertion that adds a new arena node. It ensures lower nibble positions point to lower arena addresses, maintaining a breadth-first-like layout without requiring an explicit `optimize()` call. Uses `swap_arena()` to rotate nodes into position.

## SIMD Divergence Scan

`simd_find_divergence()` replaces the nibble-by-nibble scalar scan for key comparison during insertion. It compares N bytes at a time using `Simd::<u8, N>`, finds the first differing byte via `simd_ne` + `first_set`, then resolves the diverging nibble with a branchless XOR trick (`diverging_nibble`). A scalar `find_divergence()` call handles the tail. The `from` parameter lets the caller skip already-confirmed-matching prefix bytes, halving comparison work in deep tries.

## Lookup

```
node = arena[0]
loop:
    if node.prefix_len >= max_nib:
        if node.is_terminal(): verify key == buf[node.raw_offset..], return index
        return None
    nib = key_nibble_at(key, node.prefix_len)
    slot = node.children[nib]
    if slot == 0: return None
    if node.is_leaf(nib): verify key == key_slice(slot), return index or None
    node = arena[slot]
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

`optimize()` rewrites `buf` so that keys appear in sorted order with contiguous layout:

1. Pre-allocates `new_buf` at exact size (`vec![0u8; buf_len]`).
2. DFS walk copies keys via `copy_from_slice` (no per-key `extend_from_slice` overhead).
3. Updates `index[].0` offsets and `node.raw_offset()` fields.
4. Truncates to reclaim space from removed keys.

After `optimize()`, a forward iteration hits `buf` in ascending memory order — sequential, prefetcher-friendly access. Idempotent — second call is a no-op.

## Benchmarks

See `benches/bench_results.md` for current numbers.

## Todo

High Impact

1. **remove()** — Currently impossible. Needs to handle: leaf removal, node collapse when a node drops to 1 child, and updating `leaf` fields on ancestors. The `leaf` field invariant ("always points to a descendant") breaks on removal unless propagated upward. Also need to clear `terminal` when removing a prefix key.

Medium Impact

2. **Drop the dummy key** — The `index[0] = (0, LEN::zero())` dummy costs an allocation and makes `get_value` do `-1` offset math. Instead, use `+1` encoding consistently for `children[]` leaf slots. The `leaf` field can still use direct indices since it's always set on creation.

3. **Iterator sentinel cleanup** — Replace the `usize::MAX` sentinel with a bool flag on the stack or a separate "initialized" state. Cleaner and avoids the `if nib == usize::MAX` branching.

4. **seek() uses next() as fallback** — When the exact-match leaf key is less than the seek key, we call `self.next()` which rebuilds the stack. We could instead just advance within the current node.

5. **Reorder Node fields** — `children` is the first field (offset 0), which is good for cache locality since it's accessed first in lookup. Consider whether `prefix_len` and `leaf_mask` should be packed into a single u32 (they're both u16) to save 2 bytes of padding, though this doesn't affect the overall 80-byte size due to alignment.

Low Impact / Code Quality

6. **Debug visualization** — A Dot or Debug impl that renders the trie structure would make debugging much easier.