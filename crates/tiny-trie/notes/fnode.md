# FlatNode (Fnode) Implementation Plan

Status: **Phase 0 DONE. Step 2 DONE. Step 3 DONE (`flat_get` + read path: `get`/`get_unchecked` dispatch Fnodes; `NibbleIter` `Frame` enum with Fnode frame + `fnode_seek`; terminal+branch handled). Step 4 REVISED encoding DONE on the read path (`base`+`terminal`+relative-u8-offset, CAP 8→16, root-terminal restriction LIFTED; 199 tests green incl. `fnode_read_terminal_root` for the fallback path). `flatten()` DONE as a STANDALONE pub method — the trie now PRODUCES Fnodes itself: rebuilds the arena (counts Vec + rebuild passes), collapsing any non-root subtree with ≤`FNODE_CAP` keys and ≥2 Inodes into one Fnode at its root's arena slot (consuming the old child Inodes — no orphans); `walk_optimize` remaps Fnode `base`+offsets so `optimize`→`flatten`→`optimize`→`flatten` cycles converge. Measured: 1860 random keys → arena 47344B→25168B (~47% drop), 159 Fnodes. Wiring `flatten()` INTO `optimize()` is DEFERRED: it makes `optimize` emit Fnodes, but `insert` (step 5) is still Inode-only and panics on an Fnode — needs the `fnode_mode::OptimizeOnly` expand-on-write path first. Next: step 5 (`flat_insert` + `flat_split` + `Case`/`bump_walk` extensions, incl. the expand-on-write gate so optimize can flatten), then step 6 promote/demote, step 7 bench contestant.**
Plan file (approved): `/home/d/.claude/plans/buzzing-giggling-hedgehog.md`
Memory: `flatnode-size-decision.md`, `fnode-read-path.md`

## Motivation

Nibble trie memory is "horrendous" — ~186 B/key at 1M keys (`notes/_summary.md:235`).
The node arena is ~60% of memory and **occupancy falls with depth**: deep subtrees are chains
of sparse 16-slot `Node`s, most slots empty. A **FlatNode (Fnode)** collapses one small/deep
subtree (≤ `CAP` keys) into a single dense node storing a flattened **pre-order micro-trie**:
per slot a `prefix_len` (discriminating depth), a nibble (packed), and a key-index ptr
(`None` = branch marker, `Some` = terminal key index into `index`). One Fnode replaces many
sparse Inodes.

The trie is **insert-only** (no remove API — confirmed: no `fn remove/delete/pop` in
`nibble_trie.rs`). So subtrees only grow: Fnodes are produced by compaction and **split into
INodes on overflow**; never merge. If insert perf regresses too badly, Fnode creation is gated
to `optimize()`-only (expand-on-write otherwise).

## Step 4 design (REVISED): base + terminal + relative-offset Fnode

The step-2/3 Fnode (CAP=8, `nibbles:u32`, one `OptNz<PTR>` key-index ptr per slot) is
**superseded** for step 4 by a denser encoding that raises capacity to **16 keys in one cache
line** and lifts the step-3 "subtree root can't be terminal" restriction. The step-3 read
tests/helpers are rewritten to this encoding as the **first task of step 4** (before
`flatten()`/`optimize()`).

### Why change it
At 6 B/slot (`OptNz<u32>`+`u16`), a 16-slot Fnode is ~105 B — bigger than the 76 B Inode,
defeating the zero-bloat goal. The key insight: **`index` is kept in sorted key order** (insert
places each key at its sorted position `compute_p` and shifts successors right), so a subtree's
keys appear in `index` in increasing position order. Store the leftmost key's absolute index
once as `base`, and every other key as a **u8 offset from `base`** (`key_index = base + offset`).
That collapses the 4 B ptr → 1 B offset, paying for CAP=16. Keys still live in `buf`, pointed to
by `index`, pointed to by trie nodes — **no change to key storage** (no side buffer, no dense
region): the offset is an `index`-position distance.

### New struct
```rust
const FNODE_CAP: usize = 16;   // 1 base (leftmost) + 15 array slots

pub struct FlatNode<PTR: TrieIndex, LEN: TrieIndex> {
    pub nibbles: u64,                      // 15 nibbles × 4 bits (slots 0..14)
    pub base: PTR,                         // index into `index` of the leftmost (reference) key
    pub prefix_lens: [LEN; FNODE_CAP - 1], // 15 discriminant depths for slots 0..14
    pub offsets: [u8; FNODE_CAP - 1],      // key_idx = base + offset; 0xFF = NULL / branch marker
    pub count: u8,                         // live array slots (0..=15)
    pub terminal: bool,                    // whether `base` (the subtree root) is itself a terminal
}
```
- **`base`** = the leftmost key's `index` position. Doubles as the reference key (same role as
  `Inode.leaf`) for `simd_check_prefix`. Its discriminating depth is `parent.prefix_len` (the
  parent already matched the edge nibble there), so it is **not stored** and **not a slot**.
  Pulling the leftmost out of the array (instead of duplicating it as slot 0) is what makes
  `flat_split` symmetric — no slot-0 special case; the new Fnode's leftmost becomes its
  `base`+`terminal`, the rest fill its array.
- **`terminal`** = whether the leftmost/root key is itself terminal. `true` + deeper slots =
  **terminal+branch root**; `true` + no deeper slots = pure-leaf root; `false` = pure-branch
  root (`base` is reference-only). **This lifts the step-3 root-not-terminal restriction** — the
  root gets its own representation outside the slot array.
- **Array slots 0..count-1** = the non-leftmost keys, each `(nibble, prefix_len, offset)`.
  `offset ≠ 0xFF` → terminal key at `base + offset`; `offset == 0xFF` → pure branch marker (its
  children follow as deeper slots). Because `base` is the smallest index in the subtree, real
  offsets are ≥ 1.

### Byte budget (PTR=u32, LEN=u16)
`nibbles` 8 + `base` 4 + `prefix_lens` 15×2=30 + `offsets` 15 + `count` 1 + `terminal` 1 =
**59 B → 64 B padded** (`u64` forces 8-align). One cache line; **< Inode 76 B** for the u32/u32
bench combo. Fnode now holds 16 keys in 64 B ≈ 4 B/key structural (vs ~9.5 for the CAP=8 step-3
Fnode).

### Why u8 offsets are unconditionally safe (no guard needed)
NibbleTrie is **insert-only** (no `remove`/`delete`). Without deletion, `index` density stays in
50%–90%: `optimize()` re-spreads to the `2*i+1` layout (1 gap/key) and the `>90%` trigger
re-spreads. A ≤16-key subtree therefore spans at most ~32 `index` slots → offsets ≤ ~32, far
under `0xFF`. **No flatten guard / split-on-overflow needed now.** (If deletion is ever added:
add a `span ≤ 254` flatten-guard and a split-on-overflow trigger.)
- Bonus: after every `optimize`, the subtree's keys occupy consecutive odd slots
  `2i+1, 2i+3, …` → offsets become canonical even values `0,2,4,…`.

### Consequence: relative offsets need maintenance under shifts
Because offsets are *relative to `base`*, the index-maintenance passes update `base`+`offsets`,
not absolute per-slot indices:
- **`bump_walk` (step 5):** on a physical shift at point `p` (keys with index ≥ `p` move +1):
  `base` bumps by 1 iff `base ≥ p`; each slot's `offset` changes by `[base+offset ≥ p] −
  [base ≥ p]` — computable purely from the encoded `(base, offset)`, no per-slot key-index
  lookup. The only non-trivial case is `base < p ≤ base+offset` (a contiguous run stops
  mid-subtree): `base` stays, that slot's offset +1.
- **`optimize`:** already walks the whole trie DFS and re-spreads `index` to `2i+1`; re-derive
  each Fnode's `base = min subtree key index` and `offset = key_index − base` along the way
  (→ canonical even offsets).
- **`flat_get`:** the parent already matched the edge nibble at `parent.prefix_len = P`, so the
  leftmost (`base`, depth P) is already "consumed." Scan the array slots (all depth > P) with
  the same descend/land logic as step 3; on **no array match**, land on `base` — `simd_eq`
  verify, return `base` iff `terminal`. (The exhausted-query / prefix-key cases fall out of
  "no array slot is reachable at the query's remaining depth.")

### NibbleIter Fnode frame (revised)
The frame enumerates terminals in sorted order: **position 0 = `base` iff `terminal`**, then
array slots 0..count-1 with `offset ≠ 0xFF` in order (skip `0xFF` branch markers). So
`Frame::Fnode` carries either a "base" mark or a slot index; `current`/`advance_next`/
`fnode_seek` walk base-then-slots. `fnode_seek` lower-bound: first of {`base` (if terminal),
array terminals} whose key ≥ seek.

### Sizing re-measure (Phase 0 follow-up)
Fnode is now 64 B with 8-align. Re-run `flat_node_sizes` for the exact `ArenaNode` tag math: for
**u32/u32** the enum stays ~Inode-sized (Fnode 64 ≤ Inode 76; the 8-align may cost a few bytes
vs the old 76, but B/key drops sharply). For **u16/u16** Fnode 64 > Inode 40 → enum bloats to
64; B/key still improves (64/16=4 vs 40/8=5) but every Inode slot pays 64. Use the
**union / separate-arena fallback** (in "Risk" below) for compact modes if that hurts. Update
`flat_node.rs` experiment types to the new layout for the size test.

## Phase 0 result (DONE)

Ran `flat_node_sizes` `#[ignore]` test. **To re-run**: build the test binary and run it
directly (cargo's stdout is filtered by the rtk hook):
```
BIN=$(cargo test -p tiny-trie flat_node_sizes --no-run --message-format=json 2>/dev/null \
      | grep -o '"executable":"[^"]*"' | head -1 | cut -d'"' -f4)
"$BIN" flat_node_sizes --ignored --nocapture
```

Results (bytes):
```
u64 nibble pack:
  (u32,u32) Inode=76  CAP8 F/E/U=80/88/80  CAP12=112/120/112  CAP16=144/152/144
  (u16,u16) Inode=40  CAP8 48/56/48        CAP12 64/72/64      CAP16 80/88/80
  (u8,u16)  Inode=22  CAP8 48/56/48        ...
u32 nibble pack (CAP<=8):
  (u32,u32) CAP8  Inode=76  Fnode=72  Enum=76  Union=76   <- zero bloat
  (u16,u16) CAP8  Inode=40  Fnode=40  Enum=44  Union=40   <- union zero bloat, enum +4
  (u8,u16)  CAP8  Inode=22  Fnode=40  Enum=44  Union=40
```

### Decision: **CAP = 8, `nibbles: u32`, `enum ArenaNode { Inode, Fnode }`** (step 2/3; SUPERSEDED for step 4 — see "Step 4 design (REVISED)")

> **Step 4 revises this to CAP=16, `nibbles: u64`, `base`+`terminal`+relative-u8-offset** (16
> keys in 64 B, root-terminal lifted). The CAP=8/`OptNz`-per-slot form below is the step-2/3
> record; step 4 rewrites it.

- For the bench combo (u32,u32): Fnode 72 B fits inside Inode's 76 B; the enum tag hides in
  Inode's 3 bytes of padding → `ArenaNode` = **76 B = Inode size, zero bloat**. The "blows up
  to 152" gate is NOT triggered, so the untagged-union fallback is not needed for the main case.
- `nibbles: u64` forces 8-byte align on the whole Fnode → bloat. `u32` holds 8 nibbles (8×4=32
  bits), drops Fnode to 4-align, lets it fit inside Inode. **CAP is capped at 8 for u32
  nibbles**; higher CAP needs u64 → bloat. 8 is the sweet spot for the ≤80 B target.
- Compact u16/u16: enum = 44 B (+4/slot). The union (40, zero bloat) saves 4 B/slot but needs
  `ManuallyDrop` + high-bit-PTR tag plumbing (unsafe access). **Defer the union**; revisit only
  if compact-mode memory matters. u8/u16 bloats (22→44) but u8 tries are tiny-scale.
- Fnode holds ≤8 keys in 72 B ≈ 9 B/key structural for a dense leaf-pack (vs ~76 B/key for a
  sparse Inode chain). Subtrees >8 keys split into multiple Fnodes under an Inode.

### Phase 0 files already in the tree (NOT wired into the trie)

- `crates/tiny-trie/src/tiny_array.rs` — TinyArray copied from `crates/btrees/src/tiny_array.rs`
  (interim local copy; extract to shared crate later). Self-contained std-only; relies on
  `generic_const_exprs` (already enabled in `lib.rs`). Has `insert_at`/`remove_at`/`push`/`pop`/
  `truncate`/`permute_in_place`/`drain_into`/`drain_into_front`/`drain_front_into`. No `T: Copy`
  bound; `Drop` owns initialized slots. Added a `Default` impl.
- `crates/tiny-trie/src/flat_node.rs` — `FlatNode<PTR,LEN,const CAP>` (`nibbles: u64`),
  `ArenaNode` enum, `UntaggedArenaNode` union, plus the **N32 experiment variants**
  (`FlatNodeN32`/`ArenaNodeN32`/`UntaggedArenaNodeN32`, `nibbles: u32`) used only by the size
  test. `lib.rs` has `mod tiny_array;` and `pub mod flat_node;`.
- `crates/tiny-trie/src/tests/nibble_trie.rs::flat_node_sizes` — `#[ignore]` size table.

**Integration uses `FlatNode` with `nibbles: u32` + fixed `CAP = 8` (drop the const-generic CAP
and the N32/union experiment variants from the integration path; keep them only as test
fixtures, or delete once the real `FlatNode` is u32/fixed-CAP).**

## Data model

> **Step 4 revises the `FlatNode` shape** to `base`+`terminal`+relative-offset (CAP 16) — see
> "Step 4 design (REVISED)". The CAP=8 `OptNz`-per-slot struct below is the step-2/3 record;
> `ArenaNode` (enum `Inode`/`Fnode`) and the root-always-Inode / not-`Copy` points still hold.

`arena: Vec<ArenaNode<PTR,LEN>>` where the current `Node` becomes the Inode variant:

```rust
// Final integration form (CAP fixed = 8, u32 nibble pack):
const FNODE_CAP: usize = 8;

pub struct FlatNode<PTR: TrieIndex, LEN: TrieIndex> {
    pub nibbles: u32,                       // 8 nibbles × 4 bits
    pub slots: TinyArray<(OptNz<PTR>, LEN), FNODE_CAP>,  // (key_idx_or_None, prefix_len)
    // ptr=None → branch marker; Some → terminal key index into `index`
    // len lives inside TinyArray
}

pub enum ArenaNode<PTR: TrieIndex, LEN: TrieIndex> {
    Inode(Node<PTR, LEN>),    // unchanged 16-slot direct-addressed node
    Fnode(FlatNode<PTR, LEN>),
}
```

- Fnode is a **DAG leaf**: slots hold only key indices, never arena refs. Multi-level
  internally via pre-order `prefix_len` (the scan algorithm below).
- Inode `children[nib]`: `leaf_mask` bit set → key index; else arena index into the enum arena
  — `match &self.arena[idx]` decides Inode vs Fnode. **No new discriminator field.**
- Invariant: **root (`arena[0]`) is always an Inode** — keeps `arena.is_empty()` entry checks
  simple; Fnodes only appear below the root.
- `ArenaNode` is **not `Copy`** (Fnode has `Drop` via `TinyArray`). See Copy→borrow refactor.

## The flat scan algorithm (lookup)

> **Step 4 revises this** for the `base`+`terminal`+offset encoding: the leftmost key is pulled
> out of the array into `base` (depth = `parent.prefix_len`, already matched by the parent), so
> the scan skips it and walks only the 15 array slots (all depth > P); on no array match it
> lands on `base` (verify `simd_eq`, return `base` iff `terminal`). The pseudocode below is the
> step-3 (CAP=8, slot-0-is-leftmost) form — left in place as the descend/land logic reference;
> step 4 adapts it per the "Step 4 design (REVISED)" `flat_get` bullet.

The flattened node is a pre-order DFS of a path-compressed micro-trie. `prefix_len[i]` is the
discriminating depth (NOT the key end — a key can discriminate early and carry a long suffix in
`leafptr`). Nibble checks are navigation only; the stored key is the source of truth.

**Critical insight**: `d == L-1` is NOT "the query lands there." The discriminant depth `d` is
where the key diverges from a sibling, not where it ends. The real "lands there" = matched at
`d` AND no child whose discriminant lies in `(d, L)`. Descending past a terminal is safe: if a
terminal at `d` had `key == query` (length `L`), its key would go through any child branch at
`d' < L`, so it'd live under that child, not at `d`.

```text
fn flat_get(node, key, L):
  i = 0
  depth = node.slots[0].prefix_len        # first entry's discriminant depth
  if depth >= L: return None
  while i < node.len:
    d = node.slots[i].prefix_len
    if d < depth: break                   # surfaced above frontier
    if d > depth: { i += 1; continue }    # in a subtree we haven't descended into
    if key[d] != nibble(node, i):
      i += 1                              # (or skip subtree: while slots[i+1].pl > d)
      continue
    # on path. can the query descend further?
    can_descend = i+1 < node.len
        and node.slots[i+1].prefix_len > d
        and node.slots[i+1].prefix_len < L
    if can_descend:
      depth = node.slots[i+1].prefix_len
      i += 1
    else:
      # landed — query can't go deeper. ptr tells terminal-ness.
      match node.slots[i].ptr:
        Some(ptr) => return if index[ptr].key == key { Some(ptr) } else { None }
        None       => return None
  return None
```

`nibble(node, i) = (node.nibbles >> (4*i)) & 0xF`. Full-key check uses the existing `simd_eq`
against `index[ptr]` (handles compressed middle + post-discriminant suffix; "append anything,
tree stays the same" property). Reuse `key_nibble_at`, `key_slice`/`index`.

### Bugs to avoid (from the design turns)
1. Loop must start at `i = 0` (entry 0 is a real branch, not a header).
2. No early `return ptr` before the full-key check — the landing `match` IS the return and
   always verifies via `simd_eq`. Path compression means bytes between discriminant depths were
   never compared.
3. Don't carry a `child` variable for branch markers — `ptr` (`OptNz`) handles terminal-ness
   directly (`None` = branch marker, `Some` = terminal).

## Read path changes

- `get()` / `get_unchecked()` (`nibble_trie.rs:600`, `:644`): on descent `match
  &self.arena[phys]`; `Inode` → existing logic; `Fnode` → `flat_get(node, key, max_nib)`.
  Root-always-Inode invariant keeps the `arena.is_empty()` entry check valid.
- `NibbleIter` (`:1558`, used by `Cursor::seek` and `bump_walk`): `descend_first` /
  `push_next_child` / `current` / `advance_next` gain an **Fnode frame** `(arena_idx,
  slot_cursor)` that enumerates the Fnode's terminal entries in pre-order (== sorted) order,
  integrating with the existing stack advance/backtrack. Fnode is a DAG leaf so the frame just
  walks slots (skip `None` branch markers; emit `Some` terminals in order).
- Public `Cursor` (`:1762`, linear scan over `index`) is **unchanged** — flat structure is
  transparent to it because `index` stays sorted by invariant.

## Insert

- `find_insert_case` (`:1002`): on an Fnode child, resolve either `Case::FlatInsert` (key lands
  inside the Fnode) or handle divergence above the Fnode at the Inode parent as today. New
  `Case::FlatInsert` and `Case::FlatSplit` variants (add to the `Case` enum at `:838`).
- `execute_case(FlatInsert)`: in-place Fnode insert per `notes/_summary.md:343-350` — scan
  prefix_lens, compare stored keys from `prefix_len..end` to find the insert position, find the
  next `None` to know the shift count, `TinyArray::insert_at` to shift, compute the new entry's
  `prefix_len` from divergence vs the immediately-previous child, bitshift `nibbles` at `4*i`
  (insert nibble: `nibbles = (nibbles & mask_lo) | ((nibbles & mask_hi) << 4) | (nib << 4*i)`).
  Handle the **leftmost edge case** (new leftmost changes entry 0 and the parent's leftmost
  `leaf`) — `notes:351` was cut off; this is the tricky case.
- `execute_case(FlatSplit)` on overflow (len would exceed `FNODE_CAP`): per `notes:334-340`,
  split the Fnode into a new Inode holding the shallowest level's nibbles as children, each
  child a fresh Fnode (or leaf/Inode) built from the grouped entries. Replace `arena[phys]`
  (the Fnode) with the new Inode; push child Fnodes.
- `bump_walk` (`:1203`): extend to enter Fnodes and bump key-index ptrs in their slots when a
  shifted key lives in an Fnode (mirrors the Inode leaf-child bump). **With the step-4
  relative-offset encoding:** `base += 1` iff `base ≥ p`; each slot `offset += [base+offset ≥ p]
  − [base ≥ p]` — computable from encoded `(base, offset)` alone (no per-slot key-index lookup).
- `compute_p` / `right_anchor` / `subtree_successor` (`:1104`, `:1151`, `:1171`): an Fnode's
  leftmost = its first `Some` slot ptr (entry 0 by pre-order); successor-after-Fnode resolved at
  the Inode parent level.
- **Perf gate**: a strategy flag `fnode_mode: { Always, OptimizeOnly }`. `Always` = insert
  into/split Fnodes normally. `OptimizeOnly` = an insert that would touch an Fnode first
  expands it back to INodes (expand-on-write), so insert code stays on the Inode path; Fnodes
  only appear after `optimize()`. Start `Always`; flip to `OptimizeOnly` if the insert bench
  regresses unacceptably.

## optimize() / flatten()

- `optimize()` (`:733`, existing 2n+1 buf/index respread) extended to handle Fnode variants in
  `walk_optimize` (`:758`): remap their slot key indices like Inode leaf children; Fnode
  leftmost = slot[0] ptr. Idempotence preserved.
- New `flatten()` pass (called at the end of `optimize()` and optionally standalone): DFS the
  arena; for any subtree with ≤ `FNODE_CAP` keys and ≥2 Inodes (so it actually saves memory),
  emit one Fnode in pre-order with correct `prefix_len`/nibble per entry and key-ptr = the
  remapped index; reparent the parent's child slot to it. Keep root an Inode.

## promote / demote

- `ArenaNode::promote`/`demote` dispatch: `Inode` → existing `Node::promote`/`demote`
  (`:291`, `:314`); `Fnode` → widen/narrow `PTR` across all slots (and capacity-check).
- `NibbleTrie::promote`/`demote` (`:1517`, `:1530`) map over the enum arena.

## Copy → borrow refactor (mechanical, required)

`Node: Copy` today; `ArenaNode` is **not** `Copy` (Fnode has `Drop` via `TinyArray`). Every
copy-by-read site `let node = self.arena[i];` must become `let node = &self.arena[i];` (or
`.clone()` where a value is truly needed). Known sites: `walk_optimize` (`nibble_trie.rs:766`),
`bump_descend_first` (`:1253`); audit the rest with `grep -n "self.arena\["`. `Node` itself
stays `Copy`; only the enum wrapper isn't.

## Risk: enum-arena size bloat (RESOLVED for u32/u32 by Phase 0)

A homogeneous `Vec<ArenaNode>` sizes every slot to the largest variant. Phase 0 showed CAP=8 +
u32 nibbles → `ArenaNode` = 76 B (u32/u32), zero bloat. Levers if compact/u8 modes ever matter:
1. Untagged `union` + high-bit-PTR tag (user proposal) — saves the enum's tag byte; sizes to
   `max(Inode, Fnode)` like the enum. u16/u16 → 40 (zero bloat), u8/u16 → 40. Needs
   `ManuallyDrop` + unsafe access + high-bit PTR plumbing (u8 halves flat capacity to 128).
2. Separate arenas (`arena: Vec<Node>`, `flat_arena: Vec<FlatNode>`) + an Inode `flat_mask: u16`
   (parallel to `leaf_mask`) — no Inode bloat at all; the fallback if compact-mode memory matters.

## Tests (`crates/tiny-trie/src/tests/nibble_trie.rs`)

- Extend the oracle: `recompute_leftmost`/`verify_invariants` (`:948-1022`) handle Fnodes —
  Fnode leftmost = first `Some` slot ptr; subtree key count = count of `Some` slots; add a
  `verify_flat_invariants` (pre-order `prefix_len` monotonicity, `nibble == key[prefix_len]`
  for each entry, entries sorted, `len` correct).
- Size assertions: `size_of::<ArenaNode<u32,u32>>()` = 76, `size_of::<FlatNode<..>>()`,
  `size_of::<Node<..>>()` (Inode) for the three PTR widths.
- New unit tests for `flat_get`, in-place `flat_insert`, and `flat_split` directly.
- Stress: insert → flatten → insert-more cycles and insert-into-overflowing-Fnode, all cross
  checked against `BTreeMap` via `cross_check_oracle` and `verify_invariants` after every
  insert (`stress_insert_sequence` pattern, `:1049-1068`). No `rand` dep — use the existing
  xorshift64 `next_u64` (`:930`).

## Bench

- Add `NibbleFlatBench` (and optionally `NibbleFlatOptBench`) in
  `crates/benches/src/nibble_trie.rs` following `NibbleOptBench` (`:49-97`); register in
  `all_contestants()` (`crates/benches/src/main.rs:600-620`) + the `use` block (`:257`).
  Interface is the unchanged `TinyTrieMap` trait (`type NT = NibbleTrie<Vec<u8>, usize, u32,
  u32>` — note `u32` LEN, not the default `u16`).
- Compare B/key (`bench_memory`), insert, lookup, fwd/rev iter vs `NibbleOpt`/`NibbleTrie`.

## Files

- `crates/tiny-trie/src/nibble_trie.rs` — enum arena, `FlatNode` (u32 nibbles, CAP=8),
  `flat_get`/`flat_insert`/`flat_split`, `NibbleIter` Fnode frame, `bump_walk` Fnode bump,
  `Case` new variants, `flatten()` + `optimize()`/`walk_optimize` Fnode handling,
  `promote`/`demote` dispatch, Copy→borrow refactor.
- `crates/tiny-trie/src/tiny_array.rs` — DONE (copied).
- `crates/tiny-trie/src/flat_node.rs` — DONE (experiment); fix to u32 nibbles + fixed CAP=8
  for integration.
- `crates/tiny-trie/src/lib.rs` — `mod tiny_array;` `pub mod flat_node;` DONE; re-export
  `FlatNode`/`ArenaNode` once finalized.
- `crates/tiny-trie/src/tests/nibble_trie.rs` — oracle + size asserts + new tests.
- `crates/benches/src/nibble_trie.rs`, `crates/benches/src/main.rs` — contestant.

## Verification

1. `cargo test -p tiny-trie nibble_trie` — existing suite (192 tests) + new flat tests green,
   including the extended invariant oracle on flattened tries.
2. Run the bench binary (`crates/benches`): confirm B/key drops vs `NibbleOpt` and that
   lookup/iter don't regress; check insert vs the `fnode_mode` flag.
3. If compact/u8 enum bloat ever matters, switch to the union or separate-arena fallback.

## Implementation order (current status)

1. **Phase 0** — DONE: size experiment → **CAP=8, u32 nibbles, enum**.
2. **DONE**: Integration `FlatNode<PTR,LEN>` (`nibbles: u32`, fixed `FNODE_CAP = 8`) and
   `ArenaNode<PTR,LEN>` enum (`Inode`/`Fnode`, with `promote`/`demote` dispatch) defined **inside
   `nibble_trie.rs`** (the Phase-0 `flat_node.rs` experiment types are left intact as the
   `flat_node_sizes` test fixtures — `use super::*` brings the integration `ArenaNode` into the
   test module, the size test still imports the experiment `crate::flat_node::*`). Arena swapped
   to `Vec<ArenaNode<PTR,LEN>>`; `inode()`/`inode_mut()` Inode-only accessors added (panic on
   Fnode — none produced yet); every arena read/push/promote site refactored (Copy→borrow; the
   two genuine copy-out sites `walk_optimize` and `bump_descend_first` copy the inner `Node:
   Copy` via `*self.inode(phys)`). `Node` pushes wrapped in `ArenaNode::Inode`. Suite green:
   `cargo test -p tiny-trie` = **190 passed, 3 ignored** (incl. `flat_node_sizes`). No Fnodes
   produced.
   - **`trie-stats` unlinked:** `crates/benches/trie-stats.rs` (standalone structure-analyzer
     binary) walked `trie.arena` with direct `Node` field/method access and broke on the
     `ArenaNode` swap. Its `[[bin]]` target in `crates/benches/Cargo.toml` is now commented out
     (file kept on disk, like the `crates/archive` precedent — `trie-stats.rs` is at the crate
     root so autobins won't rediscover it). The `bencher` runner and `tiny_trie_bench` lib build
     clean; `cargo build --workspace --all-targets` = 0 errors. Restore + update it (mirror the
     test oracle's `ArenaNode::Inode` match) when Fnode stats are needed.
3. **DONE**: `flat_get` — the pre-order micro-trie scan (descend on nibble match
   iff the next slot is strictly deeper & the query hasn't exhausted; otherwise
   land and verify the `Some`-ptr terminal by full-key `simd_eq`). `get()` /
   `get_unchecked()` dispatch internal children that are `ArenaNode::Fnode` to
   `flat_get` (parent Inode is the descent point; root-always-Inode invariant
   keeps the entry check valid). `NibbleIter` refactored to a `Frame<PTR>` enum
   (`Inode { encoded, mask, nib }` | `Fnode { arena_idx, slot }`): `descend_first`
   positions at an Fnode's first terminal; `current`/`current_index` read the
   terminal key index; `advance_next` walks `Some`-ptr slots in order (skip
   `None` branch markers) and pops to the parent Inode on exhaust;
   `backtrack_to_next` skips leftover Fnode frames; `seek` dispatches an Fnode
   child to `fnode_seek` (first terminal ≥ key, or backtrack to the parent's
   next child — parent frame already on the stack from `seek`'s descent push).
   The bump_walk init stack mapping (`nibble_trie.rs:~1184`) converts `Frame` →
   `(usize, u16, usize)` and panics on an `Fnode` frame (insert-into-Fnode is
   step 5; step-3 tests never insert into an Fnode trie).
   - **Invariant:** the Fnode's subtree *root* must not be terminal — its
     terminal key has no edge-slot inside the Fnode (the edge to the root lives
     in the parent Inode), so `flat_get`'s `depth >= max_nib` entry check would
     miss it. `flatten()` (step 4) avoids terminal-rooted subtrees. **Non-root
     terminal+branch nodes ARE handled** (a key that is a prefix of deeper keys
     in the subtree): encoded as a `Some`-ptr edge slot (the terminal key) with
     its children's entries following at greater depth, `flat_get` descends
     past it when the query continues (`can_descend` true) and lands on it
     (returning the terminal, verified by `simd_eq`) when the query is exhausted.
     A `None`-ptr slot is a pure (non-terminal) branch. (The original pseudocode
     in `notes/_summary.md` covers the same case: it sets `child = children[i]`
     on each nibble match and verifies `index[child].key == query` at the end.)
   - **Tests** (`tests/nibble_trie.rs`): `flatten_subtree_to_fnode` /
     `collapse_to_fnode` / `first_fnode_candidate` / `best_fnode_candidate`
     helpers build Fnodes by flattening a non-root Inode subtree (pre-order DFS,
     rejecting a terminal root / Fnode-children / `>FNODE_CAP`; a terminal
     internal child's edge slot carries `Some(child.leaf)`); `cross_check_fnode`
     checks `get`/`get_value`/`get_unchecked`/forward-iter/`seek`-lower-bound vs
     a `BTreeMap`. Tests: `fnode_read_single_level` (4-leaf),
     `fnode_read_multi_level_descent` (2-branch+4-leaf, exercises `can_descend`),
     `fnode_read_prefix_key` (`{aa,aaaa,aaab,b}` — terminal+branch "aa"),
     `fnode_read_branch_then_leaves`, `fnode_read_stress` (200 random keys,
     collapse every candidate incl. terminal+branch, cross-check after each).
     `cargo test -p tiny-trie` = **195 passed, 3 ignored**. No Fnodes produced
     by the trie yet (no `flatten()`).
4. `flatten()` + `optimize()` integration — **starts by rewriting the step-3 Fnode to the
   revised encoding** (see "Step 4 design (REVISED)"): `FlatNode` becomes `nibbles:u64` +
   `base:PTR` + `prefix_lens:[LEN;15]` + `offsets:[u8;15]` + `count:u8` + `terminal:bool`
   (CAP 8→16); adapt `flat_get` (scan skips the pulled-out `base`; land on `base` iff
   `terminal` on no array match) and the `NibbleIter` Fnode frame (base-then-slots); rewrite
   the read-test helpers `flatten_subtree_to_fnode`/`collapse_to_fnode`/`cross_check_fnode`
   for the new struct — `base` = leftmost key's `index`, `terminal` = root's terminal flag
   (root-terminal subtrees now flattenable — lift the step-3 reject), array slots =
   non-leftmost keys with `offset = key_index − base` (reject `> 15` array slots or span > 254).
   Then add the real `flatten()` pass (DFS the arena; for any subtree with ≤ 16 keys and ≥ 2
   Inodes, emit one Fnode in pre-order; reparent the parent's child slot to it; keep root an
   Inode) and wire it into `optimize()` (re-derive `base`/`offsets` after the `2i+1` respread →
   canonical even offsets). Re-run `flat_node_sizes` for the new `ArenaNode` tag math. Batch-
   created Fnodes pass the oracle.
5. `flat_insert` + `flat_split` + `Case`/`bump_walk` extensions — insert-into-Fnode and overflow
   correct; add `fnode_mode` gate.
6. `promote`/`demote` for Fnode.
7. Bench contestant + measure; tune (SIMD flat-scan over the 8-wide `prefix_len` array,
   parallel-array slots if hot).

## Key existing functions to reuse

- `simd_eq(a, b)` (`nibble_trie.rs:435`) — full-key equality at Fnode landing.
- `key_nibble_at(key, idx)` (`:533`), `key_nibble_at_unchecked` (`:551`), `nibble_count` (`:562`).
- `key_slice(key_index)` (`:573`), `index`/`buf` for full-key retrieval.
- `simd_find_divergence` (`:459`), `simd_check_prefix` (`:496`) — for `flat_insert` divergence.
- `Node::children_mask` (`:282`), `is_leaf`/`is_occupied`/`set_leaf_child`/`set_internal_child`.
- `TinyArray::insert_at`/`remove_at`/`drain_into` (`tiny_array.rs`) — Fnode slot shifts/splits.
- `OptNz<PTR>` (`:131`) — `None`/`Some` key-index ptr in Fnode slots.