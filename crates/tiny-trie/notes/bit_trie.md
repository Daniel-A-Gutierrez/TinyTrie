# Bit Trie

A binary radix trie where each decision point is a single key bit. Every internal
node has exactly two children — child[0] for bit 0, child[1] for bit 1 — stored
inline as `[u32; 2]` (same pattern as NibbleTrie's `[u32; 16]`, just 2 slots).
Because both children are always present (binary trie), there are no empty slots
and no mask needed. Extract one bit, index directly, done.

The tradeoff is deeper traversals (log₂(N) levels instead of log₁₆(N)), but each
level touches far less memory (16 bytes vs 72–80 bytes per node).

## Structure

```
Node (16 bytes):
    children: [u32; 2]    // bit 31 = is_leaf, bits 0–30 = index
    prefix_lens: [u16; 2]  // per-child prefix lengths (bit count)
    leaf: u32               // bit 31 = is_terminal, bits 0–30 = key index

BitTrie<T>:
    arena: Vec<Node>       // arena[0] = root; u32 indices replace pointers
    buf: Vec<u8>           // all keys concatenated (no null terminators)
    index: Vec<(usize, u16)> // (offset, byte_len) per key; index[0] = dummy
    values: Vec<T>         // values[i] ↔ index[i+1]
    root_prefix_len: u16   // root has no parent, so its prefix_len lives here
```

### High-bit leaf encoding

Bit 31 of each `children[i]` indicates whether the value is a leaf key index
(bit set) or an arena index (bit clear). Value 0 means empty slot (transient
during construction only). This packs the leaf/terminal flags into existing
fields, eliminating the separate `leaf_mask` byte from the old 12-byte layout.

### Terminal flag

Bit 31 of `leaf` indicates whether the node is terminal — its own key ends
at this position. Keys that are prefixes of other keys (e.g. "ab" in {"ab",
"abc"}) are represented by this flag rather than a null-byte leaf child. This
eliminates null terminators, allows `0x00` bytes in keys, and makes `get()`
accept plain `&[u8]`.

### Per-child prefix lengths

Each node stores `prefix_lens: [u16; 2]` — the prefix length (in bits) for
each child's subtree. The node's own prefix length comes from its parent's
`prefix_lens[child_bit]`. The root's prefix length is stored in
`BitTrie.root_prefix_len`. This avoids storing a per-node `prefix_len` field,
keeping the node at 16 bytes.

### Key encoding

A dummy entry at `index[0] = (0, 0)` points at `buf[0]` (empty key). Real keys
start at index 1. This allows 0 to be used as a sentinel for "empty" in
`children[]` slots. Leaves (high bit set) contain key indices into `index[]`.

### Bounded divergence scan (insertion optimization)

During insertion, at each internal node the new key is compared against the
reference key only from `confirmed` to `prefix_len` — the bounded range where
a divergence would force a node split. If the keys match through this range,
descent continues without scanning the full key. Only when a divergence is
detected (or a leaf is hit) does the full SIMD-accelerated scan run to find
the exact divergence point.

## Key bit extraction

MSB-first ordering (bit 0 = most significant bit of byte 0). This ensures the
trie's lexicographic order matches byte order — essential for correct iteration.

```rust
fn key_bit_at(key: &[u8], bit_pos: usize) -> u8 {
    let byte_idx = bit_pos / 8;
    if byte_idx < key.len() {
        (key[byte_idx] >> (7 - bit_pos % 8)) & 1
    } else {
        0 // past end of key = implicit null terminator
    }
}
```

Bits past the end of the key read as 0, so shorter keys sort before longer keys
that extend them. No null terminator needed in the key data.

## Lookup

```
if arena.is_empty(): return None
node_idx = 0
prefix_len = root_prefix_len
loop:
    node = arena[node_idx]
    if prefix_len >= max_bits:
        if node.is_terminal():
            verify key from buf → return index or None
        return None
    bit = key_bit_at(key, prefix_len)
    child = node.children[bit]
    if child == 0: return None
    if child & LEAF_BIT:
        key_index = child & !LEAF_BIT
        verify key from buf → return index or None
    else:
        confirmed = prefix_len + 1
        prefix_len = node.prefix_lens[bit]
        node_idx = child
```

## Insertion

1. **Empty trie** — root is terminal or single leaf.
2. **Bounded check** — compare new key against reference key from `confirmed`
   to `prefix_len`. If match, descend. If divergence, full SIMD scan for exact
   point.
3. **Node split** — divergence before `prefix_len`: create new parent at
   divergence point, update parent's `prefix_lens`.
4. **Leaf split** — hit a leaf child: full SIMD scan, create split node.
5. **Terminal** — key bits exhausted at a node: set terminal flag.
6. **Parent tracking** — `parent_info: Option<(u32, usize)>` tracks descent
   path so node splits can update the parent's `prefix_lens`.

## Iteration

Binary trie iteration uses a stack of `(arena_index, which_child)` where
`which_child` ∈ {0, 1, TERMINAL_POS(2), u8::MAX(before-first)}. Terminal keys
are visited before children in forward order, after children in backward order.

Navigation is separated from data access (same pattern as NibbleTrie):
- `advance_next()` / `advance_prev()` — move cursor, return `bool`
- `current()` / `current_index()` — read key/value at current position
- `next()` = `advance_next()` + `current()`
- `prev()` = `advance_prev()` + `current()`

## Memory comparison

| Metric              | NibbleTrie (u32) | BitTrie (u32)  |
|---------------------|------------------|-----------------|
| Node size           | 80 bytes         | 16 bytes        |
| Children per node   | 16 (inline)      | 2 (inline)      |
| Depth (10M keys)   | ~5–10 levels     | ~20–40 levels   |
| Leaf encoding       | leaf_mask (16b)  | high bit per u32|
| Terminal keys       | offset bit 63    | leaf bit 31     |

BitTrie is **5× denser** per node than NibbleTrie. Each level touches 16 bytes
vs 80 bytes. The question is whether the smaller working set compensates for
more pointer chasing — that's what benchmarks answer.

## Fixed-key-length optimization (future)

For fixed-size keys (e.g. hashes, IP addresses, UUIDs), the `index` array can
be eliminated entirely. Since all keys are the same length N, `leaf` and `leaf
child` values become direct byte offsets into `buf`: offset = key_index * N.
The `prefix_lens` field still handles variable-length prefix lengths within the
trie structure, but key retrieval becomes `buf[ki * N .. (ki+1) * N]` with no
indirection.

Node layout stays the same (16 bytes). The `BitTrie` struct drops `index` and
stores `key_len: u16` instead. This reduces memory per key by the size of the
index entry (6 bytes on 64-bit: usize offset + u16 len) at the cost of
requiring fixed key length at compile time or construction time.