# tiny-trie

Arena-based radix tries: `NibbleTrie` (16-way, flagship), `NibTrie` (4-way),
`BitTrie` (2-way), plus feature-gated `DynTrie` (auto-promoting) and
`FixedLenNibbleTrie`. Nightly only — `#![feature(portable_simd)]`.

`TrieIndex` (impl'd for u8/u16/u32/u64) is shared by the nibble-based tries.
`max_value_sentinel()` is kept on the trait even though `nibble_trie` uses `0`
(via `OptNz`) as its empty sentinel — siblings `fixed_len_nibble_trie` and
`nib_trie` use `PTR::max_value()` as theirs.

## NibbleTrie — non-obvious invariants

**Sparse inline-value index.** `index: Vec<Option<Slot<LEN, T>>>` where
`Slot<LEN,T> = (NonZero<usize>, LEN, T)` (buf offset, key length, value inline).
Position == key index; `None` = gap; `index[0]` is a dummy `None` so real keys
start at 1. There is **no separate `values` vector** — the value lives in the
slot. `n_keys` is the live count. **The `usize` returned by `insert` is not
stable across inserts**: a later insert's shift or an `optimize()` respread
moves earlier keys' slots. Resolve by value (`get`/`iter`), never by a captured
index.

**`OptNz<PTR>` (0-sentinel).** `#[repr(transparent)]` over `PTR`; `0` = empty,
nonzero = real index. Real arena child addresses are `>= 1` (root = arena[0],
never a child target) and real key indices are `>= 1` (index[0] dummy), so `0`
is always free. `[OptNz<PTR>; 16]` is layout-identical to `[PTR; 16]`;
`Node::children_mask` casts the array and feeds the SIMD path — this cast is
the **only `unsafe` in `Node`**.

**Terminal flag, not null terminators.** A key that is a prefix of another is
marked `terminal: bool` on the node where it ends. `0x00` bytes are valid in
keys; `get` takes plain `&[u8]`.

**`leaf` invariant.** Every node's `leaf` is the key index of the leftmost key
in its subtree. A **terminal** node's `leaf` is pinned to its own (prefix) key
and must not be overwritten by a descendant's slot — `up_walk_leftmost` breaks
at terminal ancestors for this reason (this was a real bug).

**`optimize()`.** Rewrites `buf` in DFS key-sorted order and re-spreads `index`
to capacity `2n+1` with keys at slots `2i+1` (gaps leave room for future
shifts). Arena topology is unchanged; idempotent. Triggered heuristically
(>90% density), guarded so `2n < PTR::max` (u8 tops out ~127 keys before the
guard skips optimize to avoid overflow).

**`flatten()` is separate and must be called explicitly** — it is not wired
into `optimize()`. Insert is Inode-only and panics on an Fnode; wiring flatten
in requires the expand-on-write path. `optimize`→`flatten`→… cycles converge.

## Fnode (FlatNode) — dense leaf-pack node

`ArenaNode<PTR,LEN>` is a `Copy` enum: `Inode(Node)` | `Fnode(FlatNode)`; the
arena is `Vec<ArenaNode>`. `FlatNode`: `nibbles: u64` (15 nibbles), `base: PTR`
(leftmost key's index pos, doubles as reference key), `terminal: bool`,
`slots: TinyArray<(LEN, u8), 15>`. `FNODE_CAP=16` (1 base + 15 slots); slot
`offset = key_index - base`, `0xFF` = branch marker. `terminal=true` ⇒ `base`
is the root's own prefix key (pulled out of the array, returned by `flat_get`'s
fallback); `terminal=false` ⇒ `base` is reference-only (emitted as offset-0
slot). `u8` offsets are safe because the trie is insert-only and `index`
density stays 50–90%, so a ≤16-key subtree spans ≤~32 slots.

`flatten()` rebuilds the arena top-down, collapsing non-root subtrees with
`≤FNODE_CAP` keys and `≥2` Inodes into one Fnode. **Known regression, not
fixed**: the enum-arena sizes every slot to `max(Inode, Fnode)`; for the u32/u32
bench combo `Fnode` (~144 B) > `Inode` (76 B), roughly doubling arena memory. The
agreed fix (separate `flat_arena` for Fnodes, Inodes back to `Vec<Node>`) is
**not applied** — the enum-arena design is still in place.

## Iteration API

- `Cursor` (`iter`/`iter_last`) is a **linear scan** over the sparse `index`
  (skipping `None` gaps), correct because the index is sorted by invariant.
  `current()` is a pure field read (cached `(&[u8], &T)`); `seek` positions via
  the internal `NibbleIter` tree walker then resumes the scan. Returned `&'a T`
  outlives the cursor borrow. `iter()` parks before-first; `iter_last()` on last.
- `CursorMut` (`iter_mut`) is **lending** — `(K::Borrowed<'_>, &mut T)` tied to
  `&mut self`. This is a soundness requirement, not style: a re-positionable
  cursor can revisit a slot, so an `'a`-tied `&mut T` would alias.
  `materialize` does three sequential disjoint borrows (index offset/len, buf
  key, mut index value).
- `Range` (`range`/`range_bounds`): two O(keylen) seeks resolve
  `[start_pos, end_pos)`; scan bounded by `pos < end_pos` (usize), no
  per-element key compare. `Iterator` + `DoubleEndedIterator`.
  `range(impl RangeBounds<&'q [u8]>)` — bounds must be `&[u8]`.

## Key types

`ByteKey` (`key_store.rs`): `bytes()`, `from_bytes()` (owned, allocates),
`as_borrowed()` + `Borrowed<'a>` GAT (zero-alloc iteration view). Impls:
`Vec<u8>` (`Borrowed = &'a [u8]`), `String` (`Borrowed = &'a str`;
`as_borrowed` uses `from_utf8_unchecked` — sound by the contract that stored
bytes came from `String::bytes()`). `TrieKey` + `KeyStore` backends
(`BufKeyStore` flat-buffer for `Vec<u8>`, `VecKeyStore<K>` for any `TrieKey`),
1-based with index 0 dummy, are used by `BitTrie` (which keys via `K::Store`,
not its own buf); `NibbleTrie`/`NibTrie`/`FixedLenNibbleTrie` manage their own
`buf`.

`NonNullKey`, `NonZeroBytes`, and `U64Key` do **not** exist — older memory
references them but they were removed. Only `ByteKey` and `TrieKey` are
exported.

## Sizes (asserted in tests)

- `Node<u32,u16>`=76 B, `Node<u16,u16>`=40 B, `Node<u8,u16>`=22 B.
- `NibNode<u32,u16>`=28 B (4-way, `PTR::MAX` sentinel, has `occupancy` +
  `leaf_mask` + `terminal` fields; no stacking).
- `BitTrie::Node`=16 B (high-bit leaf/terminal encoding in `children`/`leaf`,
  per-child `prefix_lens`, root's length in `BitTrie.root_prefix_len`).
- `FixedLenNode<u16>`=40 B, `<u32>`=76 B (no `LEN` generic, no `offset` field —
  computed from `leaf * max_len`; `flags:u8` bit0=terminal; `PTR::max_value()`
  sentinel; `lens: Vec<u16>` tracks real key lengths).

## Other

- `DynTrie<T>`: enum dispatch over `NibbleTrie<Vec<u8>, T, {u8,u16,u32,u64}, u16>`,
  auto-promotes on `near_capacity` (`arena.len() >= PTR::max` || `index.len()
  >= PTR::max`). **No STAK** — the old STAK const-generic was fully removed;
  `NibbleTrie` has 4 params (`K, T, PTR=u32, LEN=u16`, `K: ByteKey`).
- `FixedLenNibbleTrie`: `insert_auto` auto-optimizes at power-of-two sizes.
- `TinyArray<T, N>` is a `Copy` fixed-cap inline array (`len: u8`, no `Drop` —
  `T: Copy`); copied from `crates/btrees` as an interim local copy.