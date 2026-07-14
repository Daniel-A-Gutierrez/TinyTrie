# doa — Dense Ordered Arenas

Consider a btree that mallocs each node. Three costs: 8-byte pointers between
nodes; nodes scattered across RAM (cache-hostile, no prefetch); and
serializing to/from disk is painful — no clean node→offset mapping unless you
page-align, and even then deciding which node goes where in the file is
non-trivial.

doa is an alternative: store an ordered sequence **contiguously in blocks,
addressable by small pointers** (i8+). A subtree is one `block_id` (a `usize`)
containing nodes that reference each other with i8/i16 internal pointers; a node's
value can itself be a `block_id` forwarding to another block/tree. Contiguous
storage — even with `None` gaps between elements — means iteration is a linear
scan the CPU prefetches aggressively, and stays fast at large sizes;
serialization is writing the contiguous bytes.

The crate preserves the **ordering** of the sequence through mutations, not the
*pointers*. Pointers move when a block reorganizes (spread/split on exhaustion);
that change is **reported via `InsertDelta`**, not hidden, so the consumer can
remap. The consumer enforces whatever structure-level invariant it needs
(preorder for a binary tree so leftmost-descent is a sequential scan;
prefix-chasing for a radix trie) on top of the stable ordering. Even with no
tree at all — "node" = key — the stable ordering makes this a sorted array with
~O(log n) insert instead of O(n).

It exposes two tiers:

- **Block** — a fixed-width-addressable run of the sequence. `try_insert_*` /
  `remove` return a `Result` / `InsertDelta` describing what moved or what
  failed; the *consumer* decides how to respond to exhaustion. A block will not
  split or shove items onto other blocks on its own — by design. A consumer may
  use blocks directly, keeping their own `Vec<Block>` as their arena.
- **Arena** — automatic block management. The arena runs adaptive **strategies**
  so blocks optimize for the workload at runtime, dodging n² insertion and
  address exhaustion; `arena.insert_*` is effectively infallible. This is where
  log(n) insert lives — the block layer alone can't promise it.

**Status — work in progress, currently does not compile.** `block.rs` is
mid-refactor of the address model (`addr_range` / `ptr_root`); `strategy.rs`
and tests still call removed functions (`max_magnitude` / `half_ptr` /
`assert_capacity` / `Block::new`). `strategy.rs` is in particular malformed —
the description below is the **intended design**, not always the current code.
Fix the seam before trusting any signature.

## The block

A `Block<T, PTR = i16, const OVERP = false>` is one contiguous run of the
sequence, addressed by `PTR` (i8..isize). Storage is `VecDeque<Option<T>>` — a
slot per addressable position, `None` where a gap sits.

### Address layout

A virtual address maps to a physical slot by `phys = (virt + virt_offset)
>> addr_shift`; inverse `virt = (phys << addr_shift) - virt_offset`. Two knobs:

- **`addr_shift`** spreads physical slots across the signed address space
  (`PTR::MIN..=PTR::MAX`) — stride `1 << addr_shift`. **Spreading does not
  increase capacity** — an i8 addresses 256 positions regardless. It trades
  dense packing for *headroom* at the ends, so appends/prepends have addresses
  to grow into before the block must reorganize.
- **`virt_offset`** slides the window — the mechanism that keeps existing
  addresses stable when the physical buffer shifts (notably `push_front`).
- **Address bounds are the full type range** (`PTR::MIN()..=PTR::MAX()`, e.g.
  i8 = `-128..=127`). The range includes `MIN`, so negatives are *generated* (by
  `push_front` negating a positive offset) and **never mirrored**: never compute
  `-addr` for an address — `i8::MIN` isn't negatable. Derive the low bound from
  `MIN()` directly, not `-MAX()`.

### The sparse AP — aligned positions

The **AP** (*aligned positions*) is the stride-spaced grid of slots `find_slot`
walks when looking for a reusable `None`. A slot is on the AP iff
`(phys + v_off_phys) & none_mask == 1`; its stride is `none_stride = none_mask
+ 1`, independent of the address stride `1 << addr_shift`. `None` gaps are
pre-stocked on the AP so the walk is a short bounded stride-hop, not a scan.
**The gaps are the whole mechanism** separating O(n) `Vec`-insert from cheap
mid-insert: a mid-insert reuses a nearby AP `None` instead of shifting a tail.

### Mutation surface and the `InsertDelta` contract

- `push_back` / `push_front` are address-stable. `push_front` bumps
  `virt_offset` by `1 << addr_shift`, which cancels the physical shift
  `VecDeque::push_front` causes — stable for *all* `addr_shift`, not just 0.
  (Don't "simplify" to `+= 1`; a regression test locks it.)
- `try_insert_before` / `try_insert_after` go through `find_slot` + the
  strategy; `remove` takes a slot and may shift the resulting `None` to a nearby
  aligned position. All return an **`InsertDelta`**:
  - `Free` — placed in a pre-existing `None` (or pushed); address-stable, no remap.
  - `Move` — elements shifted; carries `addr_delta` (`minus << addr_shift`),
    the remap the caller applies to its pointers.
  - `BlockSplit` — placeholder (see Arena).
- The consumer reads the delta to fix up its pointers. **The block does not
  shift-to-create a gap on insert, and does not split** — those are the
  consumer's / arena's job.

### Returns a `Result` by design

`try_insert_*` / `push_*` can fail with `NotFound::OutOfBudget` (no gap within
stride-budget) or `AddressExhaustion` (next address not representable in `PTR`).
A bare block surfaces the failure and lets the caller decide — it will not
overstep into a split or a shove. A consumer using blocks directly is the one
who knows the structure's semantics, so the decision is theirs.

### Strategies — why they exist

A block's address layout is a **bet on its workload**. The four strategies are
four bets; each optimizes for one insertion pattern and loses on another. The
arena (at the arena tier) picks the bet per block and changes it when proven
wrong.

**Random — the intended case.** A random-optimized block spreads its elements
across the *whole* address range so `None` gaps sit throughout. A mid-block
insert then reuses the nearest gap with a short stride-walk (`find_slot`,
budget-bounded) — O(budget), not O(n). This is the win: insert anywhere without
shifting a tail.

> *State:* i8 block, cap 8, len 4, stride 32 (`addr_shift 5`, `v_off = 128`).
> Spread from a dense 4-element block left the elements at the **even** phys
> slots — `Some` at `[-128, -64, 0, 64]` (phys 0,2,4,6; phys 0 stays put since
> `2*0 = 0`) — and opened the **odd** phys slots as AP gaps — `None` at
> `[-96, -32, 32, 96]` (phys 1,3,5,7). Insert near `0` (phys 4): `find_slot`
> walks the AP outward, finds the `None` at `-32` (phys 3) or `32` (phys 5)
> within budget → places there. Cheap; no tail shifted.

**The flaw that motivates *append*.** The spread that makes mid-inserts cheap
spends the address range on gaps throughout, leaving **less than a stride of
headroom at the back**. So an append-heavy workload on a random block hits the
wall almost immediately:

> *State:* i8 block, random, cap 4, len 4, stride 64, addresses `[-128, -64, 0,
> 64]` (spread across the range; top at 64). *Operation:* `push_back`. Full →
> grow + spread (stride 64→32); existing addresses preserved, one new back slot
> appears at `96` (64 + 32). **One** usable append. The next `push_back` would
> need `128` — not representable in i8 (max `127`) — so the block must grow +
> spread *again* (stride 32→16), an O(n) redistribution, to free one more end-slot
> (`112`). Each subsequent append hits the same wall and pays the same O(n)
> reorg: **O(n²) for a push_back sequence.** The layout was shaped for random
> inserts, but the workload was appends → mismatch → quadratic.

*Append's bet:* `addr_shift = 0` — elements dense at consecutive low addresses
`[0,1,2,3]`, the **entire** upper range free. Append just takes the next address
up to `PTR::MAX`; no spread, no shift, no wall. *Blind spot:* a mid-block insert
has no pre-stocked gap nearby (small budget) → `OutOfBudget` fast → expensive
mid-insert.

**Prepend** is the mirror — dense at the top, the lower range free for sustained
`push_front`. Same mid-insert blind spot as append.

**Pluripotent** is "I don't know the workload yet." A conservatively *small*
block (`cap ≤ half_ptr`) kept under the address ceiling so it can serve *either*
appends or mid-inserts for a while without reorganizing — and once the pattern is
clear it **graduates** into the matching concrete strategy. Its reason for
existing: committing early to the wrong concrete strategy costs you (the
quadratic above, or the append-block's mid-insert penalty), so it buys time to
find out.

## The arena

`Arena<T, U, I>` owns `VecDeque<Block>` (blocks linked `prev` / `next` for
ordered iteration) plus a small queue of recent insert hints. It exists to take
the decisions a bare block refuses.

- **Infallible insert.** `arena.insert_before` / `insert_after` are
  effectively infallible: when a block would return `NotFound` or
  `AddressExhaustion`, the arena responds — spread, split, or readdress — so the
  caller never has to. (Skeleton today; `insert_*` are `todo!()`.)
- **Adaptive strategies at runtime.** The arena assigns each block a strategy
  at birth and *changes it* when the workload proves the bet wrong — a random
  block getting hammered with appends is reshaped before the quadratic bites.
  This is how the arena targets log(n) insert, which the block layer alone
  cannot promise.
- **Overprovisioning.** `OVERP = true` widens `PTR` beyond the address space the
  block needs (e.g. i32 ptrs for an i8-scale block), making log(n) insert *easy*
  — but it can double the size of pointer-heavy structures (every internal
  pointer widens). A memory/insert-speed trade the arena-level consumer makes.
- **Subtrees and forwarding.** A subtree stores one `block_id: usize` and uses
  small `PTR`s *within* the block for node-to-node references; a node's *value*
  can itself be a `block_id` forwarding to another block/tree. The small-pointer
  payoff: an 8-byte malloc pointer becomes an i8 within a subtree, with one
  `usize` at the root.
- **Ordering across splits.** A split partitions the ordered run into adjacent
  blocks linked `prev` / `next`; iteration is a contiguous *logical* scan across
  the linked list even though storage is now several physical buffers. The
  linked list is the ordering-stability mechanism across splits, not just an
  iteration convenience.

## Not built / historical

**TODO:** spread / split (block-level primitives and the arena's auto-split),
graduation (`pluripotent` → concrete at `len == half_ptr`; `post_insert_check` is
a no-op stub), prepend gap-insertion, Block `iter` / `iter_mut`, trie integration.

**Historical (do not revive):** `circular_array.rs` is gone (`Block.buf` =
`VecDeque<Option<T>>`). The `MAX` const generic is gone (replaced by `OVERP:
bool`). `BlockIndex::sqrt_max` and the `BlockIndex` trait name are gone (→
`SignedBlockIndex`). Any claim spread/split or the arena are "done" describes a
prior, now-divergent session.