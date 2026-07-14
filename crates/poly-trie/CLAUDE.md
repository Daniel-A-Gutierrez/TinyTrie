# poly-trie

Graduated radix trie: nodes start as Node2 (1-bit) and graduate to Node4
(2-bit) then Node16 (4-bit). Graduation tops at Node16 — there is no Node256.

## Null-terminator contract (load-bearing)

`insert()` rejects keys containing `0x00` (`key.contains(&0)` → `Err(())`) and
appends a `0x00` terminator internally. `get()` and `seek()` require the caller
to pass a null-terminated key (this is NOT enforced — passing a bare key
silently gives wrong results). `current()` strips the trailing `0x00` from the
returned `&[u8]`. `NodeRef::Leaf.idx` is a key index, not an arena index; the
key at `keys[idx]` still carries its terminator.

## Graduation invariants (correctness-critical)

`try_graduate` promotes Node2→Node4 / Node4→Node16 bottom-up after each insert.
All four preconditions must hold or graduation is skipped:

1. All child slots occupied (no `Empty`).
2. All internal children are the same discriminant as the parent (leaves always eligible).
3. `prefix_len % new_radix_bits == 0` (alignment).
4. Every internal child dispatches at `prefix_len == parent_prefix_len + parent_radix_bits`.

The alignment invariant (precondition 3) is what makes the slot mapping
`parent_digit * factor + child_digit` bijective — no collision detection is
needed (only `debug_assert!`s guard it). Breaking alignment would corrupt the
trie silently in release builds.

## ref_keys

`ref_keys: Vec<u32>` is parallel to the arena slots. `ref_keys[node_start_slot]`
holds the key index used as the reference key for divergence checking during
insert; non-start slots within a child array hold `0` and are unused. Resized
in lockstep with `arena.capacity()` in `alloc_node`.

## Iterator sentinel

`Frame { node, slot, mask }` (24 bytes). When the trie root is a single leaf,
the frame uses `NodeRef::Empty` as a sentinel and `slot == usize::MAX` means
"before first". `mask` is a 16-bit occupancy bitmask (bit N set = slot N
occupied), recomputed on every push — `compute_mask` is scalar.

## optimize

BFS-allocates a fresh arena, remaps all internal `NodeRef` arena indices via a
`remap` table, then swaps. `Leaf`/`Empty` `idx` (key indices) pass through
unchanged — only internal-node `idx` is remapped. Frees graduation gaps.