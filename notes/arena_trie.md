# Arena Trie (Dead Code / WIP)

`src/arena_trie.rs` is 808 lines of dead code — it compiles with warnings
and is not used by any other module. The ideas explored here (variable-width
arena allocation, TableBlock/GreedyBlock/RigidBlock node types, 4-byte nodes)
evolved into the PolyTrie implementation (see `poly-trie.md`).

This file should be removed or feature-gated if not intended for future work.
The current active implementations are:

- **TinyTrie** (`prefix_trie.rs`) — DFA-based, 16-byte nodes, PairVec children
- **NibbleTrie** (`nibble_trie.rs`) — 76-byte nodes, 16-wide fanout, arena-allocated
- **BitTrie** (`bit_trie.rs`) — 12-byte nodes, binary fanout, arena-allocated
- **PolyTrie** (`poly_trie.rs`) — graduated Node2→Node4→Node16, shared arena

## Historical design notes

The arena trie explored variable-width arena allocation with different node
sizing strategies:

- **4-byte nodes**: `len + prefix_len + symbol + ptr` with 8-bit pointers limiting
  arenas to 255 addresses. The zero byte was always a leaf.
- **Block variants**: TableBlock (direct-indexed, >50% occupancy), GreedyBlock
  (first-available append), RigidBlock (structured tiers with fixed degree).
- **Node layout**: `prefix_len | len | [symbols] | [offsets]` — minimum 2-byte
  header, 4-byte word size in arena.
- **Key descent**: Finding the reference key required O(depth) descent to the
  leftmost leaf (no `leaf` field). Suggestion to store leftmost-leaf pointer
  for O(1) access was noted but not implemented.