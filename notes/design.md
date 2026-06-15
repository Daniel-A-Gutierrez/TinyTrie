# Early Design Brainstorm (Historical)

This document captures the initial design exploration that led to the current
trie implementations. The ideas evolved substantially; see the specific
notes for current state:

- `nibble_trie.md` — fixed-fanout nibble-indexed radix trie (76-byte nodes)
- `bit_trie.md` — binary radix trie (12-byte nodes)
- `poly-trie.md` — graduated adaptive-width radix trie (Node2→Node4→Node16)
- `prefix_trie.md` — TinyTrie (original DFA implementation, 16-byte nodes)
- `merging.md` — aligned graduation design (Node2→Node4→Node16)

## Original exploration summary

The project explored several approaches for representing sparse vs dense child
sets in radix tries:

1. **Flatten children/grandchildren** — compress sequential prefix bytes into a
   single node with variable-length symbols. Two variants: (A) fixed-width
   symbols with no per-symbol prefix length, (B) variable-width with SIMD
   gather + compare.

2. **Overallocate with u8 index** — reserve more slots than needed, index by
   symbol value.

3. **Grandpa's house model** — start small (Vec with u8 indices), grow to
   direct-indexed table when occupancy exceeds threshold. Eviction of subtrees
   to separate arenas when capacity is exceeded.

4. **Arena design** — Blocks and Nodes with dynamic sizing. TableBlock
   (direct-indexed), GreedyBlock (first-available), RigidBlock (structured
   tiers).

These ideas evolved into:
- **NibbleTrie**: Approach 2 (fixed 16-slot array, direct-indexed by nibble)
- **BitTrie**: Approach 2 (fixed 2-slot array, direct-indexed by bit)
- **PolyTrie**: Approach 4 (graduated sizing: Node2→Node4→Node16 with arena
  allocation and aligned merging)
- **TinyTrie**: Separate DFA-based approach with inline/pairvec children