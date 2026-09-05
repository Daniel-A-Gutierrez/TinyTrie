# doa — Dense Ordered Arenas

An alternative to malloc-per-node trees: store an ordered sequence **contiguously
in blocks, addressable by custom-width pointers** (u16/u32), as `[Option<T>]`.
Contiguous storage — even with `None` gaps — means iteration is a prefetch-friendly
linear scan and serialization is writing the bytes. The crate preserves the
**ordering** of the sequence through mutations: a contiguous run of `Some` items
may shift ±1 to make space (slide; parents' pointers are fixed up), the store may
grow and spread items over new capacity (translator remaps; vaddrs stable), and a
block may cleave in two. DOA is for **ordered, tree-like structures only** —
middle insertion requires getting the referers/referents of a moved run, which
requires a tree + a traversal state. The block interface is **unified around the
`BlockOps` surface**: no `push_back`/`push_front` at the block level — edge inserts
(before-first/after-last) are handled internally by `Pluripotent` via store edge
grows + translator compensation.

The walker hierarchy is three layers (the consumer owns node specifics, the crate
owns ordered traversal and tree ops):
1. **Consumer layer** — `NodeCursor`/`NodeWalker`/`NodeWalkerMut`, implemented on
   the consumer's own struct; masks the node representation (union/enum/whatever).
2. **Ordered traversal** — `TreeWalker<O, NW>` (wrapper, `O` phantom) + `TreeWalk`,
   one impl per ordering.
3. **Tree ops** — `TreeWalkMut` (insert/remove/fixup/slide-opening) + `SplitTreeWalker`
   (splits), over the unified `BlockOps` surface.

Solved-subtlety reasoning (why rules are what they are) lives in
[`subtle_bugs.md`](subtle_bugs.md); this file is the operational map.

## Files (lowest level → highest)

- `lib.rs` — module wiring + `Ordering`/`Order`/`RootPos` (the ordering vocabulary).
- `index.rs` — numeric trait ladder + type-level const facts underpinning all address math.
- `translator.rs` — `v2p`/`p2v` address translation, fn-ptr-specialized over zero/nonzero params.
- `metadata.rs` — the fixup protocol (`Fixup`/`Fixable`) + walker/block data types (`Pos`, `PosAncestry`, `Root`…).
- `store.rs` — unbounded `Option<MaybeUninit<T>>` slot backends + slide/find/grow/spread/split/reservation primitives.
- `blocks.rs` — `Block` (store + translator + block data + mode) + `BlockTrait`/`BlockOps` + the three modes.
- `walker.rs` — `Node`/`SplittableNode` + the three walker layers + the split driver.
- `treeblock.rs` — `TreeBlock` (param-less tree-block marker) + the `walker`/`search` free-fn constructors.
- `subtle_bugs.md` — nuanced correctness issues solved, with diagrams; the rules they left behind.
- unwired — `block_cursor.rs` + `leafblock.rs` / `inline_leafblock.rs` (compiled, dead)
  + `src/archive/` (see Historical); `examples/old_btree/` (archived consumer; the
  live consumer is `examples/btree.rs`).

## lib.rs

Module wiring + ordering markers; re-exports `metadata::Fixup`.

- `RootPos { Beginning, Middle, End }` — where the tree root lives in a fresh block.
- `Order { Pre, In, Post }` — which ordering (so tree ops `match O::ORDER` and monomorphize per-ordering *flows* that differ in steps, not just values).
- `Ordering: 'static { const ROOT_POS: RootPos; const ORDER: Order }` — impl'd by `InOrder` (Middle/In), `PreOrder` (Beginning/Pre), `PostOrder` (End/Post).

## index.rs

Numeric trait ladder + type-level facts (`MIDPOINT` neutral anchor, `ZERO`/`ONE`/`MIN`/`MAX`/`BIT_WIDTH`) + wrapping/rotate ops, macro-impl'd for the integer primitives. Foundation for all address math; upholds only the numeric contract.

- `Num` — common numeric ops + const facts + `rotate_left`/`rotate_right`/`wrapping_*`. No `Neg`.
- `SignedNum: Num + Neg` — negation + `isize` conversion.
- `UnsignedNum: Num` — `usize` conversion (direct slot indexing).
- `BlockIndex: UnsignedNum` — unsigned in-block ptr with an associated `Half` (overprovisioning sibling). Impl'd for u16 and u32 (64-bit).
- `SignedBlockIndex: SignedNum` — signed in-block ptr with `Half`.
- macros `impl_num`/`impl_signed`/`impl_unsigned`/`impl_block_index`/`impl_signed_index` — the ladder impls.

## translator.rs

Virtual↔physical address translation, specialized by fn-pointer over the 16 (inner/outer/shift/rotation × zero/nonzero) combos so a steady param is straight-line with no per-lookup branch. `v2p` is the hot path; `p2v` runs on remap.

Invariant: `p2v(p) = ((p + inner_offset) << shift).ror(rotation) + outer_offset`; `v2p` is the exact inverse. `inner_offset` lives in **physical** space (added before the shift), `outer_offset` in **virtual** space (added after). Round-trip is exact on canonical (block-handed-out) vaddrs. **Vaddrs may wrap**; the only hard rule is physical order: phys 0 = min element, phys len−1 = max.

- `AddressTranslator<P>` — `v2p`/`p2v`/`vdist`.
- `Translator<P>` — concrete translator holding two fn-ptrs; `set_*` re-specializes only on a zero↔nonzero flip, else just writes the field.
- `V2p`/`P2v` — fn-ptr aliases; `variant!`/`apply!` macros generate the 16 specialized bodies.

## metadata.rs

The fixup protocol: block ops hand back `Fixup` implementors; any tracked state (`BlockData`, walker state) that impls `Fixable` receives one and corrects the addresses it holds.

- `Fixup` — `fix_p` (rewrite phys) / `fix_v` (translate, fix, translate back) / `affects_p` / `affects_v` (skip untouched pointers).
- `GrewFixup { shl, shift_offset }` — spread remap `p → p<<shl + offset`; `{0, 1}` doubles as the plain `p → p+1` (Pluripotent front-edge grow).
- `NoneSlide` (in `store.rs`, impls `Fixup`) — one slide.
- `TwoSlide { a, b }` — two non-overlapping slides from one `find_2_slots`, composed as ONE fixup call (order-independent). The applying side still slides separately — the run-parent walks interleave with the slides and cannot compose (subtle_bugs.md §9).
- `SwapFixup { from, to }` — a swap moved the record at `from` into the None at `to`. Swaps emit no self-fixup: the mover applies it by hand; `split_root` returns it as the old-root→new-root remap for external vaddr holders. `identity(p)` for no-op remaps.
- `Fixable<P>` — `fixup(&mut self, f, tr)`; implementors fix every address they hold. Blanket impl for `()`.
- `CursorState<P>: Fixable<P> + Clone` — the walker-state seam: `position`/`reposition` required + `descend(parent, child_idx)` (the descent record; **no-op default** — descent is the cursor's only way to move). The ascent side is deliberately NOT a state hook: where parent knowledge lives is per-shape, so `ascend`/`parent` are consumer methods on `NodeWalker`. `Fixable` is a supertrait (on the assoc, where the bound propagates).
- `Pos(pub usize)` — bare position, the stackless cursor state (CursorState only).
- `PosAncestry { pos, ancestry }` — the standard stackful state (CursorState with the real descend record); consumers embed it instead of reimplementing the fixup loop.
- `HasRoot<P>: Fixable` — block data exposing a movable root phys + tree height (`root`/`set_root`/`height`/`set_height`; root promotion bumps the height, the consumer's `is_leaf` reads it).
- `Root { root, height }` — minimal tree block data (Fixable + HasRoot; not a walker state — a `set_position` on block data would rewrite the block root).
- `Ancestor { parent, child }` / `Ancestry` — the stackful walker's per-level records (+`push`/`pop`/`last`/`len`/`is_empty`).
- `Height`/`Depth` — pointer-free no-op-fixable level counters, components not states.

## store.rs

Unbounded slot backends (`VecStore`/`DequeStore` — cap grows on demand; the addressable limit lives in `Mode::MAX_CAP`, not the store). Slot primitives, spread, split, slide, find, reservation.

Invariants: `occupied ≤ len ≤ cap`; `push_*`/`grow_*`/`spread` operate on logical slots; `find_slot`/`slide_none` honor a `pin` (kept out of the moved run). **Slots are `Option<MaybeUninit<T>>`**: the discriminant is the occupancy flag (store-internal, flipped only by `alloc`), the payload is exempt from validity until written — the **alloc-write-read contract** (a slot is read only after its reservation's write completes; the exclusive `&mut MaybeUninit<T>` enforces the ordering). Both stores impl `Drop` (`assume_init_drop` over `Some` slots — `MaybeUninit` never drops `T` on its own); dropping a store with a pending reservation is UB (the contract's one sharp edge). See subtle_bugs.md §7.

- `Store<'a,T>` — slot-backend surface: `get`/`get_mut`/`slot`/`slot_mut`/`get_disjoint_mut`/`swap`/`push_front`/`push_back`/`pop_front`/`pop_back`/`grow_front`/`grow_back`/`grow`/`spread`/`free(i) -> T`/`split`/`iter`/`occupied`/`len`/`cap`/`new`/`with_capacity`/`from_vec`/`into_vec` + the reservation surface: `alloc(i) -> &mut MaybeUninit<T>` (reserve a None slot: flip + occupied, hand back the write place) and `alloc_disjoint_mut(a, b) -> (&mut T, &mut MaybeUninit<T>)` (occupied node + freshly reserved disjoint write place — the drain handoff).
- `find_slot(pos, dir, budget, pin)` — DIR-biased, budget-bounded, pin-clamped nearest-None (the run between anchor and None is Some-dense).
- `find_nearest_slot` — bidirectional-outward variant (minimizes slide distance).
- `find_2_slots(pos_a, dir_a, pos_b, dir_b, budget, pin) -> Option<TwoSlide>` — two reservations whose slides apply independently in either order; away-pointing design: sphere scan (radius `(|a−b|−1)/2`, disjoint by construction) then one requested-side call per anchor (a fallback slide preserves the walk-order side), with `slides_interfere` as the lone-None detector; interference ⇒ None ⇒ caller spreads.
- `slide_none(ms, pin)` — rotate the run `[lo,hi]` so the `None` at `from` lands at `to`.
- `spread(offset)` — double `len`, element `i → 2i + offset`.
- `NoneSlide { from, to, delta }` — a slide; impls `Fixup` (`fix_p`: `p += delta`).
- `VecStore<T>` — Vec-backed. `DequeStore<T>` — wrap-aware (cross-slice logic for find/slide/spread/split at the wrap boundary).
- `NearestNone`/`SomeIter`/`dual_scan_outward`/`slides_interfere` — scan internals.

## blocks.rs

`Block` = store + translator + block data, carrying a `Mode` by type. Two surfaces: `BlockTrait` (shared read + basic mut) and `BlockOps` (the per-mode slot surface — a trait, not inherent methods, so the tree-ops layer can call it generically). `Mode` owns the store type and the initial translator params. Inherent on `Block`: `new`/`from_parts`/`into_parts`/`insert_root`/`iter`.

Invariants: `find_slot`/`find_2_slots` re-translate `pos`/`pin` after a grow (vaddrs stable, phys remap via the returned composed `GrewFixup`); every find/slide applies its fixup to the block's **own** `BlockData` before returning (a bare `grow_and_spread` does not — its caller applies the fixup); physical order (phys 0 = min) is preserved by every op.

- `Mode<'block, P, N>` — assoc `S: Store`, consts `INNER_OFFSET`/`OUTER_OFFSET`/`SHIFT`/`INIT_CAP`/`MAX_CAP`, `make_translator`. Consts are *initial* params (vaddrs may wrap; offsets come into play at splits).
- `Uniform` — no-pin full-range (VecStore, `SHIFT = BIT_WIDTH`); for trees that grow by splitting.
- `Anchored<O>` — root pinned at `O`'s fixed vaddr (VecStore); `find_slot`/`slide_none`/`find_2_slots` implicitly pin `v2p(root_vaddr)` — the root never moves.
- `Pluripotent` — sparse both-ends-growable (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`). Edge inserts: *after-last* = `grow_back(1)`; *before-first* = `grow_front(1)` + `outer -= 1<<shift` (vaddr holders stay valid; phys holders get `GrewFixup{0,1}`); exhaustion guard `len < min(MAX_CAP, (1<<BIT_WIDTH) >> shift)`. `find_slot` order: budgeted scan → spread + rescan → edge grow.
- `Block<'block, N, P, M, D, O>` — the concrete block; `UniformBlock`/`AnchoredBlock`/`PluripotentBlock` aliases.
- `BlockTrait<'block>` — assoc `N`/`P`/`S`/`BlockData: Fixable`/`O: Ordering`; `store`(+`mut`)/`translator`(+`mut`)/`data`/`data_mut`/`set_data`, `get`/`get_mut`/`vget`(+`mut`)/`get_disjoint_mut`, `first_vaddr`/`last_vaddr`, `v2p`/`p2v`/`vdist`, `occupied`/`len`/`cap`, `free`/`swap`/`swap_open`, + the reservation surface (`alloc(OpenSlot)`/`alloc_disjoint_mut(a, OpenSlot)`).
- `BlockOps<'block>: BlockTrait` — `find_slot(pos, after) -> FoundSlot`; `find_2_slots(pos_a, dir_a, pos_b, dir_b) -> Result<Found2Slots, InsufficientMaxCapacity>` (default ladder: scan → spread + rescan → exhausted; Anchored overrides to pin its root; Pluripotent's edge-grow not tried by the default); `slide_none`; `grow_and_spread -> Result<GrewFixup, InsufficientMaxCapacity>`; `cleave(at)`; `cleave_and_rotate(v_start, v_end)`; `cleave_and_spread(at)` (default; shift-budget variant of rotate). Impl'd once per mode (disjoint by `M`).
- `FoundSlot { grew, slide }` — composed grow fixup + pending slide; `slide == None` ⇒ exhausted (caller must split). `Found2Slots { grew, slides: TwoSlide }` — the two-slot analogue.
- `OpenSlot(usize)` (`Copy`) — a `None` slot opened for insert (physical). `InsufficientMaxCapacity` — the grow-fail error.
- `grew_step`/`root_vaddr`/`fr_params` — internal helpers (grow remap application; Anchored pin target + initial params).

## walker.rs

`Node`/`SplittableNode` (the node contract) + the three layers. `B` is a trait param at every level; `O` is never a param — it is always `B::O`. **DEGREE ≥ 3** (a full node needs ≥2 keys to split into two non-degenerate halves + a separator).

- `Node { K, V, P, DEGREE, STORES_PARENTS, Payload }` — the node identity. `STORES_PARENTS: bool` gates the reparent machinery (false shapes pay nothing — the const check folds away); `Payload` = what a split promotes besides the separator (B-tree internal = the median's V, B+ inode = `()`).
- `SplittableNode: Node + Sized` — `new_root(r_v) -> Self` (the promoted root, **pre-wired with child 0 = the old root** — absorbing the only separator-less wire) + `split(&mut self, slot: &mut MaybeUninit<Self>) -> (K, Payload)` (drain the right half directly into the reserved block place). The split machinery binds `Node<P = B::P>`.
- **Layer 1** `NodeCursor<'block, B>` (`B: 'block`; ref returns use the
  `<'b> where 'block: 'b` form) — assoc **`State: CursorState<B::P>`**
  (per-implementor: stackless cursors pick `Pos`, stackful walkers pick
  `PosAncestry`) + `state()`/`state_mut()` + the shape set:
  `block`/`is_leaf`/`child_count`/`child(idx)` (panics on a node that can't
  support the op — the crate's generic code gates on `is_leaf()` first)/
  `children()` (default iter)/`lookup(k) -> (usize, Ordering)`
  (**position + comparison**: `pos` = the child `search` descends to by default;
  `cmp` = `k` vs that child — `Less` ⇒ new child at slot `pos`, `Equal`/`Greater` ⇒
  slot `pos+1`; `(len−1, Greater)` = append)/`search` (root→terminal descent;
  **required, no default** — the `Equal` interpretation is a per-shape routing
  policy). Defaults: `position`/`current`/`descend` (child → `v2p` →
  `CursorState::descend` record + reposition). No constructors on the traits —
  construction lives on `From` bounds at the free fns.
- `NodeWalker<'block, B>: NodeCursor` — `ascend`/`parent` **required** (parent knowledge is per-shape: a stackful state pops/peeks its records, a parent-pointer tree reads the node's stored field; a stackful ascend must consume a record where a pointer ascend must not, so no default serves both). `parent`'s child-idx feeds the fixup path's `set_child`. The only panic: ascending past the root.
- `NodeWalkerMut: NodeWalker` — `parts() -> (&mut State, &B)` / `parts_mut()`
  (**one call returning the pair** — the split-borrow the fixup path needs),
  `block_mut`/`has_space`/`set_child(up, child_idx, ptr)` (ancestry-aware,
  position-stable)/`set_parent` (no-op for parent-free shapes)/
  `insert_child(child_idx, k, payload, ptr)` (node-level wire — primitives only)/
  `remove_child(child_idx) -> (Option<K>, Option<Payload>, P)`.
  Defaults: `set_position`/`current_mut`/`reparent_children(v)`/
  `adopt_node(p, parent_v)` (finish a fresh/moved node: parent field + children)/
  `reparent_run(ns)` (**post-slide**, position-based over the shifted run —
  mid-fixup it would descend through just-rewritten post-slide entries over the
  pre-slide layout; subtle_bugs.md §3)/`swap_current(open) -> OpenSlot`
  (the chokepoint for "a node with children moved": state follows via
  `SwapFixup`, parent entry repointed, children reparented; **the block root is
  not updated — tree-level callers with `HasRoot` do it**).
  The `STORES_PARENTS` obligations, all const-gated: slides ⇒ `reparent_run`
  (in `apply_slide`); swaps ⇒ `swap_current`; fresh/moved nodes ⇒ `adopt_node`
  (Y at every split site — its drained children name X; R at every root
  promotion — it demotes under NR; the new node at every insert).
- **Layer 2** `TreeWalker<O, NW>` — wrapper carrying `O` as **phantom data** (per-ordering `TreeWalk` impls sit on distinct types and pass coherence; `O` bound to the block's at every use). `pub nw` field + `new(nw)`.
- `Suggested` (`Copy`) — the anchor plan (pure choice, no walking): `Parent { before }` (anchor = the current node) / `Child { idx, before }` (descend `idx`, then `subtree_first`/`subtree_last` by side).
- `TreeWalk<'block, NW, B>` — `next`/`prev`/`first`/`last` (at_root + a subtree edge)/`subtree_first`/`subtree_last`/`suggest_insertion(child_idx)`/`suggest_split()` (the split's target-slot anchor, standing on the split node; childless X → `Parent{after}` for all three). One impl per ordering.
- Ordering semantics:
  - **preorder** node-first — `subtree_first` = the node, `subtree_last` = rightmost
    leaf; suggest: gap 0 → after the parent, mid gap → before `child(k)`,
    append → after the rightmost leaf.
  - **in-order** — a node sits between `child[b-1]` and `child[b]`,
    `b = min(cc, DEGREE/2)` (`in_boundary` — **fixed by DEGREE, not cc**: a full
    node's boundary is its kept-left-half's edge, so splits never move the split
    node; cc ≤ DEGREE/2 ⇒ after-all); a gap AT b → the parent's own anchor (side
    by whether the boundary grows), else the adjacent child's edge leaf; both
    subtree edges are outermost leaves.
  - **postorder** node-last — `subtree_last` = the node, `subtree_first` = leftmost
    leaf; suggest: childless → before the parent, gap 0 → before child 0's
    subtree, gap k → after `child(k−1)`.
- **Layer 3** `TreeWalkMut: TreeWalk` (crate impl, `B::BlockData: HasRoot` — the hop
  may move the block root):
  - `insert_child(k, payload, node) -> Result<P, InsertErr>` — `has_space` →
    `lookup` slot → `suggest_insertion` → anchor walk → `find_slot` → grew fixup →
    `apply_slide` → `alloc` → ascend → `adopt_node` → node-level wire → **in-order
    parent hop iff `child_idx < DEGREE/2 && cc > DEGREE/2`** (a left insert shifts
    the boundary identity exactly like a left split; the hop's `BlockExhausted`
    leaves the tree position-invalid — cleave-before-hop is arena future work).
  - `remove_child(idx) -> (N, OpenSlot)` — unwire + free, no fixups.
  - `fixup(&NoneSlide)` — run-parent-fixup **before** the slide: forward-only walk
    of the run; per moved node `set_child(1, idx, post_v)` (idx from ancestry) +
    `set_parent` when the parent moved; position-neutral via **State
    snapshot/restore**. The walk is the **walk==slot-order canary**: after `steps`
    walk steps the position must be the run's far edge exactly (always-on
    `assert!` — a reserved-but-unwired Some is skipped → the end lands short/long,
    the tripwire for would-be `assume_init` UB; subtle_bugs.md §6).
  - `apply_slide(ns)` — **THE slide chokepoint**: fixup walk → `slide_none` → state
    fixup → `reparent_run`.
  - Slot opening (insert machinery the splits borrow): `walk_to_anchor(sug)` /
    `back_from_anchor(levels)` / `open_after()` / `open_suggested(sug)` /
    `hop_current()` (in-order relocate: `open_suggested` + `swap_current` +
    **block-root pointer maintenance when the hoppee is the root**; subtle_bugs.md §4).
  - Generic over `O` via the where-clause supertrait trick (same coverage as
    per-ordering impls, no body copies).
- `InsertErr { NodeFull, BlockExhausted }` — caller splits (a level up / the root) or cleaves and retries; `BlockExhausted` is the arena tier's cleave hook.
- `SplitWalkerExt` (crate machinery, declared as a trait so the impl can name `B`):
  - `open_split_slot()` — slot at `suggest_split`'s anchor.
  - `open_two(sug_a, sug_b)` — two slots via `find_2_slots`, both anchors walked
    and both slides computed pre-mutation (the no-outstanding-reservations rule;
    subtle_bugs.md §2).
  - `split_child_here(child_idx)` — open → drain via `alloc_disjoint_mut` →
    `adopt_node(Y)` → wire at `child_idx+1`; postorder's arm relocates X via
    `swap_current` first and drains into the `freed` token.
  - `split_into_open(child_idx, open)` — the Y=open drain+wire half.
  - `insert_new_root(open)` — place a FRESH root above the old one; pre/post swap
    it with the old root (NR under the walker's feet, root pointer/vaddr
    untouched), in-order repoints + steps the walker (R keeps its slot);
    `adopt_node(R)` everywhere.
- **`SplitTreeWalker: TreeWalkMut + SplitWalkerExt`** — `split_child(child_idx)`:
  room check → `split_child_here` → in-order hop (same rule as insert). Ends on the
  parent. `split_root() -> Result<SwapFixup, InsertErr>` per `O::ORDER`:
  - **pre** — `open_after` + `insert_new_root` (NR takes R's slot, root-first;
    identity fixup — the root vaddr is unchanged) + `split_child_here(0)`.
  - **post internal** — `open_two(suggest_split, Child{cc−1, after})`, drain R into
    y_slot, NR into r_slot + swap with r_phys (no walk in the transient window;
    subtle_bugs.md §1); identity fixup.
  - **post leaf** — Y right after R, NR right after Y (each slot written before the
    next opens; the run right of Y is empty by the root-last invariant — asserted);
    the one sanctioned `set_position`; **non-identity fixup** (the root vaddr moves).
  - **in** — R keeps its slot (its valid range is the single unchanged gap; no swap
    puts NR right of R under the walker's feet); NR's slot per its own convention
    (`b = min(2, DEGREE/2)`: adjacent-right of R when b==1, the region end when
    b==2); `insert_new_root` (explicit `set_root` + the one sanctioned
    `set_position`); non-identity fixup.

No `insert_child` name collision: `NodeWalkerMut` is impl'd on the consumer's `NW`, `TreeWalkMut` on the wrapper — different `Self` types, method sets never intersect.

## treeblock.rs

`TreeBlock` — a block whose stored type is a node. **Param-less marker trait**, crate-impl'd for `Block` per mode (`impl_tree_block!` × `Uniform`/`Pluripotent`/`Anchored<O>`).

- `TreeBlock<'block>: BlockTrait + BlockOps` (where `Self::N: Node`, `Self::BlockData: HasRoot`) — `root_position` (defaulted from `BlockData::root`).
- Free fns (construction, over the consumer's `From` impls — local type, orphan-safe): `walker::<NW>(b)` — walker at the root, `NW: NodeWalker + From<R>`; `search::<NW>(b, k)` — walker routed to `k`'s terminal, `NW: NodeCursor + From<R>` (stackless cursors work — descent only). `R` is the borrow (`&B` or `&mut B`; both `Deref<Target = B>`), so one fn covers shared and mut — the named walker type's `From` impl picks the borrow.
- `SplitTreeBlock` — declared sketch (arena-level cleave), unwired, bounds at the `BlockOps` level.

## Testing

`src/tests/` — `store.rs` (fully commented; `store.rs` still wires its `#[cfg(test)]`
module) and `block.rs` (live but unwired code against the deleted pre-refactor API —
needs adaptation, not uncommenting). No btree test file exists. When the block op
surface stabilizes, adapt the store/block invariant tests to the current API.
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Status

Compiles (lib + workspace); `examples/btree.rs` — a B+ tree consumer — runs green:
100 keys through the split driver (leaf/internal splits, multiple root promotions,
height ≥ 2), all anchor kinds, in-run + out-of-run slides.

Untested (compile-verified only, no consumer): the in-order parent hop (split and
insert triggers), the postorder split arms, the `STORES_PARENTS` reparent machinery
(`swap_current`/`reparent_run`/`adopt_node`), `find_2_slots`/`open_two`/`TwoSlide`.
The example is preorder + parent-free; an in/post consumer example, ideally
parent-storing, is the natural next test and would exercise all of them at once.

Not designed/wired: the walker-driven union-node `drop_tree` (design deferred);
`BlockExhausted` cleave handling (arena); deletion rebalancing/merges;
serialization; the test suites; `leafblock`/`inline_leafblock`.

## Historical (do not revive)

`circular_array.rs`; the `MAX` const generic; `OVERP: bool` (overprovisioning is
`BlockIndex::Half`); the old signed `BlockIndex`; `AllocStrat` + `Append`/`Prepend`
(push-only dense blocks — pluripotent covers them); `TreeOrdering`/`Sorted`; `O` as
a generic param on walkers; `RelTo` + `BPtr`/`IPtr`/`LPtr` + the `FractalForest`/
`BTree` sketches; `SS`; `Store::slots`/`slice_iter` (lying empty-iterator stubs);
`NodeWalkerMut::depth` + the state-trait `depth` (nothing read them); the crate's
`get_unchecked` pair + `Translator::set_params` (unused); `Default` on `Node`
(shape-blind — `new_root` is the shape's); `child_payload` (the wire takes
primitives); the dynamic `cc>>1` in-order mid; `boundary`/walk-back-based fixup.

## Future Work

- split hardening — in/post consumer examples (parent-storing) exercising the hop, the postorder arms, and the reparent matrix; a canary negative test (alloc-in-run-without-wire → expect the panic).
- extend the btree consumer (deletes with leaf removal via `remove_child`, underfull merges) and adapt the archived tests.
- `keys()` iter hook on the cursor so B+ shapes share the child-min fetch / separator re-derivation / equal-right routing in-crate.
- ordered iteration over K/V (`IntoIterator` on the walker + `is_leaf` filter) + a range surface.
- arena tier — infallible insert (absorb exhaustion via spread/cleave/readdress), adaptive runtime strategy switching, overprovisioning, subtrees & forwarding (block_id roots), ordering across splits.
- graduation — pluripotent → concrete strategy at len == half_ptr.
- `Fixup::applies` optimization (elide unnecessary runtime checks).
- trie integration.

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each section afterward should include a brief overview of what the file in the codebase contains, as well as what its broad purpose is, namely what invariants it maintains. Each trait and type defined in a file should be listed, 1 or 2 lines each.
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically.
Maintain this section at the end of the claude.md .