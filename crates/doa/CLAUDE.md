# doa — Dense Ordered Arenas

An alternative to malloc-per-node trees: store an ordered sequence **contiguously
in blocks, addressable by custom width pointers** (i8..isize), as [Option<T>].  
Contiguous storage — even with `None` gaps — means iteration is a
prefetch-friendly linear scan and serialization is writing the bytes. The crate
preserves the **ordering** of the sequence through mutations. A contiguous run of Some
items may be shifted left or right 1 to make space for a new T in the arena.
Two tiers: **Block** (a fixed-width run the that surfaces address / capacity exhaustion as `Result`) 
and **Arena** (automatic, adaptive, effectively infallible insert).
An arena pointer and a block pointer both must impl BlockIndex, (arena_p, block_p) uniquely identify an item
in the arena, while block_p (usually P) semi-stably uniquely identifies an item in a block.
items may be shifted by neighboring inserts but space is freed between them or before them by adjusting the block's translator params
which map virtual addresses handed out by the block to physical indeces into the underlying storage.

## Architecture

Module map of `src/`; only `index`/`translator`/`store`/`block` are realized,
`tree_traits` is mid-refactor, the rest are stubs or sketches.

- `index.rs` — numeric trait ladder (`Num`/`SignedNum`/`UnsignedNum`/`BlockIndex`/`SignedBlockIndex`) + const facts (`MIN`/`MAX`/`MIDPOINT`/`ONE`/`BIT_WIDTH`) + wrapping/rotate/halfptr ops. Macro-impl'd for i8–i64, u8–u64; `BlockIndex` (the unsigned in-block ptr trait, with an associated `Half` type for overprovisioning) impl'd for u16 and u32 (u32 on 64-bit only).
- `translator.rs` — `Translator<P>`: virtual↔physical address math via fn-ptr specialization.
- `store.rs` — `Store` trait + `VecStore`/`DequeStore`: the bounded `Option<T>` slot backends.
- `block.rs` — `AllocStrat` (four strategies) + `RawBlock`: a store + translator upholding no structural invariant; the `BlockTrait`/`BlockMutTrait` surface.
- `tree_traits.rs` — tree-tier abstractions (`TreeOrdering`/`Node`/`TreeWalker`/`Tree`/`TreeBlock`); mid-refactor, the `impl Tree` is commented out.
- `leafblock.rs` / `inline_leafblock.rs` / `abstract_tree.rs` — leaf-block experiments; stubs.
- `lib.rs` — module wiring + `FractalForest`/`BTree`/`INode` sketches (unused) + the `InsertDelta` enum (placeholder) + `RelTo` + `BPtr`/`IPtr`/`LPtr` aliases.
- `tests/` — `src/tests/{store,block}.rs`, pulled in via `#[cfg(test)] #[path = "tests/…"] mod tests;` at the end of each source file (in-crate, so `pub(crate)` internals are visible).

## Address model

How a virtual address (the `PTR` a consumer holds) maps to a physical slot
(`usize` index into the store), and the three knobs that move without
invalidating vaddrs.

- **Translation.** `v2p(v) = ((v + offset) >> shift) rotate_left(rotation)`; `p2v` is the inverse. `v2p` is the hot lookup path; `p2v` runs on remap/insert. Round-trip is exact on occupied slots (vaddrs the block handed out are canonical).
- **`offset`** slides the window; `push_front` bumps it by `1 << shift` to cancel the physical shift the store's `push_front` causes — vaddrs stay stable for *all* `shift`, not just 0. Do not "simplify" the bump to `+1`; a regression test locks it (`Pluripotent` at `shift>0`).
- **`shift`** spreads physical slots across the address range (stride `1<<shift`). Spreading does **not** increase capacity (an i8 addresses 256 positions regardless); it trades dense packing for headroom at the ends so appends/prepends have addresses to grow into before a reorg.
- **`rotation`** is the split-remap primitive (bit-rotation, not shift); `split_and_rotate` bumps it. Currently only exercised by the block-level split stubs.
- **Bounds are the full type range.** Unsigned `MIN=0..=MAX`. `push_front`'s offset walks up through `MAX` and wraps to `MIN` via `wrapping_add` — `MIN` is the exhaustion sentinel (the reserved range is spent). Never compute `-addr`; derive the low bound from `MIN`.
- **`Translator` specialization.** `v2p`/`p2v` are fn pointers picked from 8 combos of (offset, shift, rotation) zero/nonzero, so a steady param is straight-line with no per-lookup branch. `set_offset`/`set_shift`/`set_rotation` re-specialize only on a zero↔nonzero flip; otherwise just write the field.

## Stores

`Store<'a, T>` — the bounded `Option<T>` slot backend, `MAX_CAP` const (power of
two, const-asserted) bounding logical capacity. Two backends with identical
surfaces; `DequeStore` adds wrap-aware logic.

- **Capacity.** `occupied` = #Some, `len` = #slots (Some+None), `cap` = allocated. `occupied ≤ len ≤ cap ≤ MAX_CAP`. `push_*`/`grow_*`/`spread` assert against `MAX_CAP`; capacity doubles up to `MAX_CAP`.
- **Slot primitives.** `push_back` returns the new index; `push_front` shifts existing up by 1 (store-tier addrs move — the translator hides it). `insert(v,i)`/`remove(i)` require None/Some respectively (panic otherwise); both keep `len`, change `occupied` by ±1. `pop_front`/`pop_back` take an end slot, leave a gap.
- **`spread`** doubles `len`, element at `i` → `2i`, slot `2i+1` = `None`; `occupied` stable. `VecStore`: one reverse move + `set_len(2len)`. `DequeStore`: phase1 (upper half → tail) + phase2 (lower half → `2j`) for even `len`; **odd `len`** (the `len==1` pow2 base, and any non-pow2 `grow_and_spread` could hit) takes a direct reverse-move path — the phase split is invalid at `mid==0`.
- **`split` / `split_and_rotate`.** `split(at)`: `[at,len)` → new store, `occupied` partitioned. `split_and_rotate(at)`: odds-gaps both halves — left `p→2p+1`, right `k→2k+1` in a new same-cap store; evens `None`; both `len` double. Both assert the doubled lengths fit `MAX_CAP`; the block tier additionally calls these only when full.
- **`slide_none`** rotates the run `[lo,hi]` so the `None` at `from` lands at `to`; elements shift one toward `from`. `from==to` is a no-op. `VecStore`: slice rotate. `DequeStore`: in-slice rotate, or per-step swaps when the run straddles the deque's wrap boundary. A pinned slot must not be inside the run (debug-asserted; `find_slot` keeps slides off it).
- **`find_slot`** is DIR-biased (preferred side first), budget-bounded, pin-clamped (`pin<pos` raises `min`; `pin>pos` lowers `max`; `pin==pos` restricts to the DIR side only). Returns a `NoneSlide{from,to}` whose `to` is adjacent on the inserting side when the `None` is there, else `pos`. `find_nearest_slot` is the bidirectional outward variant (minimizes slide distance, dir tie-break). `DequeStore` handles the front/back slice boundary (`pos` Less/Equal/Greater than `flen`) with cross-slice fallbacks that respect the same pin clamp.
- **Iteration.** `iter` is a forward `ExactSizeIterator` over `Some` refs (`len == occupied`, skips `None`s), double-ended. `cursor` is a positioned reader (`seek` O(1), `next`/`prev`/`first`/`last` scan across gaps; `seek` to `None`/OOB panics; `position==None` at-end, from which `prev` is a no-op).

## Block & strategies

`RawBlock<'a, T, P, A, S>` = a `Store` + a `Translator`, carrying an `AllocStrat`
by type. It upholds **no** structural/tree invariant — only the address-model
invariants. `BlockTrait` is the read surface; `BlockMutTrait` the mutation
surface. A strategy is a **bet on a workload**; each optimizes one insert pattern
and loses on another. `AllocStrat` carries const params: `INIT_SHIFT`,
`INIT_OFFSET` (the anchor), `INSERT_BUDGET`, `CAP_LIMIT`, `REVERSED`.

- **`Uniform`** (random-optimized): `INIT_SHIFT = BIT_WIDTH` (full range), anchor `MIDPOINT`, `VecStore`. `try_insert_back/front` always `Err` — random-only. `find_slot` + `insert` is the path; `insert` auto-spreads at `occupied*3 > len*4` to keep gaps stocked. `len` stays a power of two. The win: a mid-insert reuses a nearby `None` within budget — O(budget), not O(n).
- **`Pluripotent`** ("don't know the workload yet"): `INIT_SHIFT = Half::BIT_WIDTH - 1`, `CAP_LIMIT = half range`, `DequeStore`. Dense `try_insert_back/front` into the free half; `push_front` bumps offset by `1<<shift` (stable for all shift). Buys time before graduating to a concrete strategy.
- **`Append`**: `INIT_SHIFT = 0`, offset `(1<<width) - half_range` (low `half_range` addresses reserved for prepend), `VecStore`. Hot `push_back` hands out dense vaddrs from `half_range` up to `MAX`; every `BUDGET`-th push pads a `None` so a stray mid-insert reaches a gap within budget. Cold `push_front` into the reserved low range via `wrapping_add`, `Err` when offset wraps to `MIN` (reservation spent).
- **`Prepend`**: `Append` mirrored — `REVERSED = true`. Hot `push_front` is physical `push_back` (front = high end); iteration is high→low; `find_slot`'s dir is flipped. Cold `push_back` into the reserved range.
- **Mutation contract.** `insert_root` lands at phys 0 = the anchor vaddr. `find_slot(pos,dir,pin)` finds a free slot or grows: on a budget miss it calls `grow_and_spread` then **re-translates** `pos`/`pin` (spread shifts phys `i→2i`; vaddrs are stable so re-translation yields the new phys). `grow_and_spread` fails at `shift==0` or `len*2 > max_capacity`; `find_slot` returns `None` at `len == max_capacity`. `Uniform::insert` computes the new vaddr **before** its auto-spread (the vaddr is stable across the spread; computing it after would point at a gap). `remove` frees a slot for reuse.
- **What the block does *not* do.** It will not split or shove items to another block on its own — exhaustion is surfaced (`Result`/`None`), the consumer/arena decides. Displaced elements' vaddrs change on a slide; only the **pin** is guaranteed stable. The remap info for displaced elements is not yet surfaced at this tier (`InsertDelta` is a placeholder in `lib.rs`).

## Testing

71 tests (`cargo test`), in `src/tests/{store,block}.rs`. They encode the
address-model and per-strategy invariants, and a reference-comparison harness
for the store's `find_*`/`slide_none`.

- **Store (VecStore + DequeStore).** Capacity/pow2 bounds; slot primitives; `spread`; `split`/`split_and_rotate`; `slide_none` compared against a reference rotation over **all** `(from,to)` pairs, contiguous and wrapped (exercises the wrap-crossing swap path); `find_slot`/`find_nearest_slot` compared against a reference over pos×dir×budget×pin, contiguous and wrapped (exercises the `flen` Less/Equal/Greater keypoints and cross-slice fallbacks); iter/cursor; a fuzz loop checking `occupied≤len≤cap≤MAX_CAP` after every mutation.
- **Block per strategy.** `new`/empty params; `insert_root` at the anchor; hot/cold paths and their vaddrs; **vaddr stability across grow/spread** (round-trip `v2p∘p2v` + `get` consistency on every occupied slot); **pin root never moves** (root vaddr and phys constant through a 30-insert sequence); `len` pow2 (`Uniform`); exhaustion returns `None`/`Err` at `max_capacity`/`MIN`; `Pluripotent` `push_front` stability (u16 **and** u32 — the `+1<<shift` regression); `Append`/`Prepend` cold-path exhaustion at 256 reserved inserts; `Prepend` iter reversed.
- **Bugs the suite caught and locked.** `find_slot` re-translate after `grow_and_spread`; `Uniform::insert` vaddr-before-spread; `Pluripotent` `push_front` `+1<<shift` (was `+1`); `Append`/`Prepend` cold-path `wrapping_add` (was debug-overflow at `MAX`); `DequeStore::spread` odd-`len` direct path (phase split invalid at `mid==0`); `DequeStore::find_slot` cross-slice pin clamp (`scan_left` lower-bounds the back scan at `min-fl`; `scan_right` falls through `front`→`back` within budget).

## Status

Compiles; 71 tests pass. The block + store + translator + index tiers are
realized. Not built / stubbed: the **arena tier** (auto-split, adaptive strategy
switching, infallible insert — `Arena` is not implemented), `tree_traits`
(`impl Tree` commented out mid-refactor), `leafblock`/`inline_leafblock`/
`abstract_tree`, and `lib.rs`'s `FractalForest`/`BTree`/`INode` sketches. The
`InsertDelta` enum in `lib.rs` is a placeholder — block `insert` returns the new
vaddr directly, and remap info for slide-displaced elements is not yet surfaced.
Historical (do not revive): `circular_array.rs` is gone; the `MAX` const generic
is gone (replaced by `MAX_CAP` on stores + `CAP_LIMIT` on strategies); the
`OVERP: bool` block generic is gone (overprovisioning is now `BlockIndex::Half`);
`BlockIndex::sqrt_max` is gone, and the old signed `BlockIndex` split into
unsigned `BlockIndex` (with an associated `Half` type) and `SignedBlockIndex`
(the signed seam).

# Future Work 
Explicit TODO list:
- spread / split — block-level primitives and the arena's auto-split.
- graduation — pluripotent → concrete strategy at len == half_ptr; post_insert_check is a no-op stub.
- Block cursor_mut / iter_mut.
- trie integration.

Arena tier (intended, currently skeleton / todo!()):
- Infallible insert — arena.insert_before / insert_after that absorb NotFound/AddressExhaustion via spread/split/readdress so the caller never handles failure.
- Adaptive runtime strategy switching — assign a strategy per block at birth and reshape when the workload proves the bet wrong (this is where log(n) insert lives).
- Overprovisioning — OVERP = true widening PTR.
- Subtrees & forwarding — block_id: usize roots with small internal PTRs; a node's value can be a block_id forwarding to another block.
- Ordering across splits — prev/next linked list so iteration is a contiguous logical scan across split blocks.
