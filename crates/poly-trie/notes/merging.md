# Node Merging (Graduation) — Design & Implementation

## What merging is

When a narrow node fills up and its children all sit at the next
bit position, it can be *merged* into a wider node that covers more
bits per dispatch. This collapses one level of indirection and reduces
trie depth without changing the logical key→value mapping.

The graduation chain: Node2 → Node4 (1-bit → 2-bit), Node4 → Node16
(2-bit → 4-bit), Node16 → Node256 (4-bit → 8-bit).

## Aligned merging (current implementation)

Graduation uses an **alignment invariant**: each node type only graduates
when its `prefix_len` is a multiple of the new radix width.

| Node type | Radix bits | Alignment requirement |
|---|---|---|
| Node2 | 1 | any position (always aligned) |
| Node4 | 2 | `prefix_len % 2 == 0` (even positions) |
| Node16 | 4 | `prefix_len % 4 == 0` (positions 0, 4, 8, ...) |
| Node256 | 8 | `prefix_len % 8 == 0` (byte boundaries) |

### Why alignment eliminates collisions and simplifies slot mapping

When a node is radix-aligned, its children at `prefix_len + radix_bits`
are also aligned for the next wider radix. The structural slot index
`parent_digit * width + child_digit` maps **bijectively** to the wider
node's slots — no two structural indices produce the same wider digit.

Proof for Node2 → Node4: A Node2 at an even position P has children at
P+1. If both children are Node2s at P+1, their grandchildren are at P+2,
which is also even. The 2-bit digit at position P decomposes as:

```
digit_at(key, P, 2) = (bit_P << 1) | bit_{P+1}
```

Since the parent Node2 dispatches on bit P and the child Node2 dispatches
on bit P+1, the mapping `parent_slot * 2 + child_slot` exactly equals
`digit_at(key, P, 2)` — guaranteed, no key lookup needed.

This extends to Node4 → Node16 and Node16 → Node256 by the same argument:
alignment ensures the structural slot indices are a correct encoding of the
wider digit.

### Graduation preconditions

All four must hold for a node to graduate:

1. **All child slots occupied** — no `Empty` children.
2. **Same-type children** — all internal children are the same node type as
   the parent (Node2 merges with Node2s, Node4 with Node4s, etc.). Leaves
   are always eligible.
3. **`prefix_len % new_radix_bits == 0`** — the alignment check.
4. **All internal children at `prefix_len + radix_bits`** — they dispatch
   at the immediately next position.

### Slot placement

**Internal children** (same type as parent): copy their children directly
into the wider node's slots using structural indices.

```
For parent slot d with width w:
  child at slot d occupies wider slots [d * factor, d * factor + w - 1]
  where factor = new_width / cur_width
```

No key lookup, no collision detection. The mapping is bijective by
construction under alignment.

**Leaves**: still need `digit_at(key, prefix_len, new_radix_bits)` to
determine which sub-slot within the group. But alignment guarantees the
result lands within the correct group, and same-type-children + full
occupancy means no two leaves can collide.

### prefix_len invariant

**Every child must have `prefix_len > parent.prefix_len`.**

After graduating a node at position P:

| Descendant type | `prefix_len` | Reason |
|---|---|---|
| Leaf | `P + new_radix_bits` | Position after the wider node's dispatch |
| Internal node (from sub-node) | unchanged | Already at `P + radix_bits` or later |

**Do NOT shift internal children's prefix_len.** They keep their original
absolute positions, which are already correct.

### Cascading

`try_graduate` walks the parent stack bottom-up. After graduating one
node, the parent entry is updated to point to the wider node, which may
make the parent eligible for graduation too (if it's the same type with
all slots now occupied by same-type children at the next position).

All three transitions are handled in a single pass:
Node2 → Node4, Node4 → Node16, Node16 → Node256.

## The cost of alignment

At most **1 extra Node2 level** per merge opportunity. If keys diverge
at a misaligned position (e.g., bit 3), a Node2 is needed at position 3,
and the next wider node can only start at position 4 (the next 2-bit
boundary). For typical byte-oriented keys, most real divergences happen at
or near byte boundaries, so the extra depth is rare.

Example: keys `\x08` (bit 3 = 1, all other bits 0) and `\x00` (all bits 0).
With alignment, we get Node2@3 → Node4@4 instead of Node4@3, adding
one level.

## Comparison: misaligned vs aligned merging

| Aspect | Misaligned (previous) | Aligned (current) |
|---|---|---|
| Slot placement (internal) | `digit_at(key, P, radix)` key lookup | `parent * factor + child` structural |
| Slot placement (leaves) | `digit_at(key, P, radix)` key lookup | `digit_at(key, P, radix)` (still needed) |
| Collision detection | Required (keys can collide) | Impossible (structural mapping is bijective) |
| Alignment check | Not enforced | `prefix_len % new_radix_bits == 0` |
| Allowed merge positions | Any bit position | Only radix-aligned positions |
| Same-type children | Not required | Required |
| Extra depth | None | ≤ 1 Node2 per merge opportunity |
| `ref_keys` needed for merging | Yes | No (for internal children) |
| Generalization to wider nodes | Same `digit_at` approach, same collision risk | Structural `parent * factor + child`, no collisions |

## Walk-through example

Insert `\x01`, `\x02`, `\x03` (null-terminated internally as `\x01\0`,
`\x02\0`, `\x03\0`):

```
\x01 = 00000001 00000000
\x02 = 00000010 00000000
\x03 = 00000011 00000000
```

### After \x01, \x02

```
Node2 @ bit 6
  [0] → Leaf(\x01)   // bit 6 of \x01 = 0
  [1] → Leaf(\x02)   // bit 6 of \x02 = 1
```

Both slots filled. Both are Leaves. Alignment check: bit 6 is even ✓.
Graduation check:
- `digit_at(\x01, 6, 2) = (0<<1)|1 = 01` → slot 1
- `digit_at(\x02, 6, 2) = (1<<1)|0 = 10` → slot 2

No collision. Graduate to Node4@6:

```
Node4 @ bit 6
  [0] → Empty
  [1] → Leaf(\x01)   // prefix_len=8 (6+2)
  [2] → Leaf(\x02)   // prefix_len=8
  [3] → Empty
```

### After \x03

\x03 has bit 6 = 1, same as \x02. Walk Node4@6 → slot 2 → Leaf(\x02).
Divergence at bit 7. Split Leaf into Node2@7 with \x02 and \x03.

Parent stack triggers graduation check on Node2@7: both slots occupied,
both Leaves. Alignment check: bit 7 is odd → **7 % 2 ≠ 0** → **no
graduation**. The Node2@7 stays as a Node2.

```
Node4 @ bit 6
  [0] → Empty
  [1] → Leaf(\x01)   // prefix_len=8
  [2] → Node2 @ bit 7
        [0] → Leaf(\x02)   // prefix_len=8
        [1] → Leaf(\x03)   // prefix_len=8
  [3] → Empty
```

This is the cost of alignment: one extra Node2 level that a misaligned
approach would have collapsed into Node4@7. The trade-off is simpler code,
no collision detection, and clean generalization to wider node types.

## Implementation checklist

- [x] Node2 → Node4 graduation (aligned, structural slot mapping)
- [x] Node4 → Node16 graduation (same pattern, 2-bit → 4-bit)
- [x] Node16 → Node256 graduation (same pattern, 4-bit → 8-bit)
- [x] Alignment precondition (`prefix_len % new_radix_bits == 0`)
- [x] Same-type children requirement
- [x] Structural slot mapping (no key lookups for internal children)
- [x] Collision detection removed (structurally impossible)
- [ ] Benchmarks vs NibbleTrie
- [ ] Key string table (replace `Vec<Vec<u8>>` with contiguous buffer)
- [ ] u16 index variant for small tries
- [ ] Serialization (write arenas as contiguous blobs, mmap-friendly)