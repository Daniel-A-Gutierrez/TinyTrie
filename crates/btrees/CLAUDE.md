# btrees

Compact B+ trees with SIMD-accelerated node search. Two independent trees live in this crate and have **diverged** in their leaf-management strategy — do not assume one from the other.

## The two trees

**`CTree`** (`src/int_btree.rs`) — despite the legacy name, handles *both* fixed-width keys (SIMD) and variable-length byte keys (`Vec<u8>`/`Box<[u8]>`) through a single generic impl dispatched by the `SearchStrategy` trait. One struct, one code path, `K: TreeKey + SearchStrategy`.

**`StrBTree`** (`src/str_btree.rs`) — variable-length byte keys only, using packed `KeySlots<L, N>` storage (dense `SmallVec<[u8; 64]>` + `lens: [L; N]`). `L: LengthType` bounds max key length (`u8`→255 bytes, etc.). Separate node types, separate cursor (with a cached `packed_off`).

### Leaf arena — the key divergence

Both keep leaves in a flat `Vec<LeafNode>` gap-arena where a gap is a `LeafNode` with `keys.len() == 0`. `n_leaves` counts live leaves; `leaves.len()` counts slots. But:

- **CTree**: arena is kept in **strict sorted physical order** — no per-leaf `prev`/`next` linked list. `Cursor` advances slot-by-slot, skipping gaps. `split_leaf` forward-scans for the next gap at/after `child_idx+1`; if the run `[child_idx+1, g)` is occupied it `rotate_right(1)`s the run into the gap and does a targeted `remap_shift_range` (+1 on the moved leaves' parent-inode child ptrs, an in-order B-tree successor walk — *not* a full arena remap). Proactive spread triggers at **90% occupancy** (`n_leaves * 10 >= leaves.len() * 9`). `relocate(true)` grows to `2 * old_len` slots (trailing gaps absorb sequential end-splits).

- **StrBTree**: arena **uses a leaf linked list** (`LeafNode.prev/next: Option<NonZero<PTR>>`). `Cursor` follows the linked list. `split_leaf` calls `claim_slot` (forward-scan from `after+1`, **wraps** to `[0, after)` if exhausted — `unreachable` if no gap, so callers must spread first). Spreads at **100% full** (`n_leaves == leaves.len()`); `relocate(true)` grows to `2 * live` slots. Rebalance **cascades** up to `REBALANCE_DEPTH = 3` (recursively rebalances a full sibling with *its* sibling in the same direction); CTree's rebalance does not cascade.

`compact()` = `relocate(false)` + `inodes.shrink_to_fit()`; `optimize()` = `relocate(false)`.

## Const generics — the cross-crate gotcha

`CTree`/`StrBTree`/`KeyNode`/cursors carry **two plain const params** `<const N: usize, const NP1: usize>` with `where [(); N]:, [(); NP1]:`. The ptr array is `[…; NP1]`. `NP1 == N + 1` is enforced by `const ASSERT_NP1: () = assert!(NP1 == N + 1)`, **referenced in `new()`** (`let () = Self::ASSERT_NP1;`) so it is actually evaluated — an unreferenced const is never checked.

Reason: a single `<const N>` with `where [(); N + 1]:` overflows rustc's well-formedness evaluation **cross-crate** (`E0275`) — any const *expression* in a pub struct's array bound can't be WF-proven from another crate. `generic_const_exprs` does not help. Plain const params normalize fine, so consumers write `CTree<K, V, PTR, N, N+1>` directly. `LeafNode`/`TinyArray`/`KeySlots` stay single-param `<const N>`.

## Pointer encoding

`Option<NonZero<PTR>>` stores arena index **+ 1** (1-based): index 0 is valid but `NonZero(0)` is `None`. `get_ptr` decodes `nz.get().as_usize() - 1`; `set_ptr` encodes `from_usize(idx + 1)`. `PTR: TrieIndex` (impl'd for u8/u16/u32/u64) decouples the pointer width from the address space.

## Trait system (CTree)

`TreeKey` (user key → `Stored` form + `Needle` borrowed form) → `StoredKey` (**sealed** via `private::Sealed`; `cmp_key`/`eq_key`/`cmp_stored` all take `buf: &[u8]`) → `SearchStrategy` (static dispatch of `find_position`/`find_upper_bound` taking `keys: &[Stored]` + `buf`).

- Fixed keys: blanket `T: FixedLenKey → TreeKey` (identity). `FixedLenKey` carries SIMD `find_position`/`find_upper_bound` (lane counts u8=16, u16=8, u32/u64=4).
- Varlen: `Vec<u8>`/`Box<[u8]>` → `Stored = KeyRef`. `KeyRef::Inline(TinyArray<u8, 14>)` for keys ≤ **14 bytes** (`INLINE_KEY_MAX`), else `KeyRef::Buf { start: u64, len: u32 }` into the shared `CTree::key_buf`. `SearchStrategy` for varlen is a **linear scan** through `StoredKey::cmp_key` (no SIMD preview system — that was removed).

`BufKey { start: u32, len: u16 }` is a legacy vestige, still re-exported but unused by the tree.

**Note:** `KeyRef`'s derived `Ord` is *not* semantically meaningful for `Buf` variants (it compares offset/length). Real comparison always goes through `StoredKey` with `key_buf`.

## TinyArray / KeySlots invariants

- `TinyArray<T, N>` (`src/tiny_array.rs`): `[MaybeUninit<T>; N]` + `len: u8` (N ≤ 255). **Owns Drop** for initialized slots — node types have no manual Drop. `truncate` does **NOT drop** (caller moves elements elsewhere during splits). `drain_into` / `drain_into_front` / `drain_front_into` are the split/rebalance primitives. `permute_in_place` (cycle-following slot swap) exists but is **not used** by current code — the unsorted-append experiment that needed it did not land.
- `KeySlots<L, N>` (`src/key_slots.rs`): packed bytes in `SmallVec<[u8; PACKED_INLINE=64]>`, lengths in `lens: [L; N]`. Key `i` starts at `sum(lens[0..i])` (O(i) offset scan; `find_position_with_offset` returns the running offset so `eq_key_with_offset`/`key_slice_with_offset` reuse it O(1)).

## Insert path

`walk_to_leaf` is `&mut self` (insert-only; reads use `find_leaf`) and **preemptively rebalances** any full node on the descent with an emptier sibling *before* splitting. `rebalance_target(s) = (N + s) / 2`. Guard: sibling must have ≥ 2 free slots (`s + 2 <= N`) so both nodes end `<= N-1`. `find_child` (internal nodes) is **upper-bound** (first separator `> needle`); `find_position` (leaves/insert) is **lower-bound** (first key `>= needle`). After a rebalance the needle may re-route to the sibling — `find_child` is re-run on the (updated) parent only when a rebalance actually fired.

## Experiments that did NOT land

The following are absent from current code (do not assume present): the `Preview<P>`/`NoPreview` SIMD-preview system, the `ArraySearch` trait on `TinyArray`, the `IntBTree`/`VarCTree`/`FixedCTree` renames (CTree is still `CTree`; the varlen tree is `StrBTree`), the unsorted-append / sort-before-split / `permute_in_place` insert path, and the block-grouped `LeafParent` leaf storage. Current insert is sorted (`find_position` + `insert_at`); current leaves are a flat arena.