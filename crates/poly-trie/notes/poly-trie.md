# PolyTrie — Graduated Radix Trie

## Core idea

A radix trie with **adaptive node sizes**. Each node is a plain array of
`NodeRef`s — no tag byte, no header, no leaf_mask. The `NodeRef` itself
carries the discriminant (node type), the prefix position, and the arena
index. Nodes graduate from 2-slot to 4-slot to 16-slot as branching increases,
collapsing chains of narrow nodes into wider ones.

## NodeRef — packed tagged index (8 bytes, #[repr(u8)])

```
 kind(1) pad(1) prefix_len(u16) idx(u32)
```

| kind | Meaning      | Radix  | Arena                      |
|------|-------------|--------|----------------------------|
| 0    | **Empty**    | —      | — (zeroed NodeRef)         |
| 1    | Leaf         | —      | keys[idx]                   |
| 2    | Node2        | 1 bit  | arena[idx..idx+2]           |
| 3    | Node4        | 2 bits | arena[idx..idx+4]           |
| 4    | Node16       | 4 bits | arena[idx..idx+16]          |

Note: Node256 was removed; graduation tops out at Node16 (nibble-width).
`kind=0` is Empty. A zeroed `NodeRef` array is a valid initial state.
No reserved arena slot, no dummy key.

## Arena allocator

`Arena<NodeRef, u32>` with block-size free lists. `alloc_n(width, NodeRef::EMPTY)`
for contiguous child arrays. Freed slots reuse same-size blocks via
`free_n`. Indices are stable — no shifting on free.

## PolyTrie struct

```rust
pub struct PolyTrie<T> {
    arena: Arena<NodeRef, u32>,
    keys: Vec<Vec<u8>>,      // no dummy; real keys start at index 0
    values: Vec<T>,
    root: NodeRef,           // includes root's prefix_len
    ref_keys: Vec<u32>,      // one per node start slot, for O(1) reference key
    len: usize,
}
```

**Key storage**: Still `Vec<Vec<u8>>` (per-key heap allocation). Migration
to flat buffer + side index (like NibbleTrie) is planned but not yet done.

**Null-terminator contract**: `insert()` rejects keys containing `0x00` and
appends a null terminator internally. `get()` and `seek()` require
null-terminated input.

## Lookup

Tag dispatch: `discriminant() → RADIX/RADIX_BITS → digit_at() → arena.get_range(idx, width)`.
Single code path for all node types.

## Aligned graduation (implemented)

Graduation uses an **alignment invariant**: each node type only graduates
when `prefix_len % new_radix_bits == 0`. See `notes/merging.md` for full
details.

- Node2 → Node4 when `prefix_len % 2 == 0`, all slots occupied, same-type children
- Node4 → Node16 when `prefix_len % 4 == 0`, same conditions
- Slot mapping: `parent_digit * factor + child_digit` for internal children
  (bijective, no collisions). Leaves use `digit_at()` for sub-slot placement.
- No collision detection needed.

## Iterator (implemented)

`PolyIter<'a, T>` with `next()`, `prev()`, `current()`, `seek()`.

### Stack frame

```rust
struct Frame {
    node: NodeRef,    // 8 bytes — internal node (discriminant determines width)
    slot: usize,      // 8 bytes — current child slot, usize::MAX = "before first"
    mask: u16,        // 2 bytes — occupancy bitmask for Node2/4/16; 0 for Node256
}
// 24 bytes total (6 bytes padding)
```

### How it works

- **Node2/4/16**: `compute_mask()` on push, `mask_next`/`mask_prev` for O(1)
  sibling navigation via TZCNT/LZCNT on the u16 bitmask.
- **Node256**: mask is 0; uses linear `scan_next`/`scan_prev` for sibling
  navigation. **This is a known performance gap** — see below.
- **Root leaf**: `NodeRef::Empty` sentinel in frame. `current()` reads root
  directly.
- **seek()**: walks trie following key digits. When a leaf < key is found,
  checks next siblings directly (doesn't call `next()`). Requires
  null-terminated keys.
- **current()**: strips null terminator from returned key slice; `values[idx]` directly (no -1 offset)
- **Backward iteration**: use `current()` + `prev()` pattern (call `current()` first, then `prev()`)

## Optimize (implemented)

`optimize()` rebuilds the arena in breadth-first order via BFS allocation
and index remapping. Frees graduation gaps. Idempotent.

## Performance comparison with NibbleTrie

PolyTrie iteration is slower than NibbleTrie. The major causes:

| # | Issue | NibbleTrie | PolyTrie | Impact |
|---|-------|-----------|----------|--------|
| 1 | **Mask computation** | SIMD vector compare + movemask (~3 instructions) | Scalar loop with `matches!()` per element, up to 16 branches for Node16 | High — every push to stack |
| 2 | **Node256 sibling search** | N/A (fixed 16-wide fanout) | Linear `scan_next`/`scan_prev` up to 256 elements per level | Very high for Node256-heavy tries |
| 3 | **Variable-width dispatch** | Single code path (always 16-wide) | `discriminant()` → 4-way match on every level | Medium — branch misprediction |
| 4 | **Digit extraction** | Fixed 4-bit nibble (shift + mask) | `digit_at()` dispatches on 1/2/4-bit; 1-bit case calls `key_bit_at` twice | Medium — especially deep binary tries |
| 5 | **Arena indirection** | `arena[idx]` → inline `Node` with children | Read `NodeRef` (8B), then `arena.get_range(idx, width)` for separate child array | Medium — 2 derefs per level |
| 6 | **Frame size** | 16 bytes `(u32, u16, usize)` | 24 bytes `Frame { NodeRef, usize, u16 }` | Low — cache pressure on deep tries |
| 7 | **Root-leaf sentinel** | No special case (root is always arena node) | `NodeRef::Empty` sentinel branch in `current()`, `next()`, `prev()` | Low — one extra branch per call |

## Implementation checklist

- [x] NodeRef — packed tagged index (8 bytes, #[repr(u8)])
- [x] Arena allocator with block-size free lists
- [x] Insert + get with SIMD divergence
- [x] Aligned graduation (Node2→Node4→Node16)
- [x] Iterator — forward (next), backward (prev), current, seek
- [x] Optimize — BFS arena reorder with index remapping
- [x] Structure report
- [x] Benchmarks — insertion, lookup, forward iteration, backward iteration
- [x] TinyTrieMap trait integration
- [ ] Parallel occupancy arrays (occ16, occ256) for O(1) Node256 iteration
- [ ] SIMD compute_mask for Node16
- [ ] Inline digit_at for 2-bit and 4-bit radix
- [ ] Seek optimization — fast path for Node256 (skip mask, use occ array)
- [ ] Key string table (replace `Vec<Vec<u8>>` with contiguous buffer)
- [ ] u16 index variant for small tries
- [ ] Serialization (write arenas as contiguous blobs, mmap-friendly)