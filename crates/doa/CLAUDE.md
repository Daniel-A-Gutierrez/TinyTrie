# doa — Dense Ordered Arenas

An alternative to malloc-per-node trees: store an ordered sequence **contiguously
in blocks, addressable by custom-width pointers** (u8..u32), as `[Option<T>]`.
Contiguous storage — even with `None` gaps — means iteration is a prefetch-friendly
linear scan and serialization is writing the bytes. The crate preserves the
**ordering** of the sequence through mutations: a contiguous run of `Some` items
may shift ±1 to make space (slide; parents' pointers are fixed up), the store may
grow and spread items over new capacity (translator remaps; vaddrs stable), and a
block may cleave in two. DOA is for **ordered, tree-like structures only** —
middle insertion requires getting the referers/referents of a moved run, which
requires a tree + a traversal state. The block interface is **unified around the
Uniform surface**: no `push_back`/`push_front` at the block level — edge inserts
(before-first/after-last) are handled internally by `Pluripotent` via store edge
grows + translator compensation.

The walker hierarchy is three layers, splitting what used to be a
node-trait × ordering permutation explosion (the consumer owns node specifics, the
crate owns ordered traversal and tree ops):
1. **Consumer layer** — `NodeCursor`/`NodeWalker`/`NodeWalkerMut`, implemented on
   the consumer's own struct; masks the node representation (union/enum/whatever).
2. **Ordered traversal** — `TreeWalker<O, NW>` (wrapper, `O` phantom) + `TreeWalk`,
   one impl per ordering.
3. **Tree ops** — `TreeWalkMut` (`insert_child`/`remove_child`/`fixup`) over the
   unified `BlockOps` surface.

## Files (lowest level → highest)

- `index.rs` — numeric trait ladder + type-level const facts underpinning all address math.
- `translator.rs` — `v2p`/`p2v` address translation, fn-ptr-specialized over zero/nonzero params.
- `store.rs` — unbounded `Option<T>` slot backends + slide/find/grow/spread/split primitives.
- `metadata.rs` — the fixup protocol (`Fixup`/`Fixable`) + default block/walker data types (incl. the standard walker state `PosAncestry`).
- `blocks.rs` — `Block` (store + translator + block data + mode) + `BlockTrait`/`BlockOps` + the three modes.
- `walker.rs` — `Node`/`SplittableNode` + the three walker layers.
- `treeblock.rs` — `TreeBlock` (param-less tree-block marker) + the `walker`/`search` free-fn constructors over consumer `From` impls.
- `lib.rs` — module wiring + `Ordering`/orderings + `RelTo` + sketches.
- disabled/stubs — `block.rs` + `block_cursor.rs` (pre-refactor reference code, not
  compiled), `examples/old_btree/` (archived consumer, all commented; the live
  consumer is `examples/btree.rs`), `leafblock.rs` /
  `inline_leafblock.rs` (dead sketches), `src/archive/`.

## index.rs

Numeric trait ladder + type-level facts (`MIN`/`MAX`/`MIDPOINT`/`ONE`/`BIT_WIDTH`)
+ wrapping/rotate ops, macro-impl'd for i8–i64, u8–u64. Foundation for all address
math; upholds only the numeric contract.

- `Num` — common numeric ops + const facts (`MIDPOINT` neutral anchor, `ZERO`/`ONE`/`MIN`/`MAX`/`BIT_WIDTH`) + `rotate_left`/`rotate_right`/`wrapping_*`. No `Neg`.
- `SignedNum: Num + Neg` — negation + `isize` conversion.
- `UnsignedNum: Num` — `usize` conversion (direct slot indexing).
- `BlockIndex: UnsignedNum` — unsigned in-block ptr with an associated `Half` (overprovisioning sibling). impl'd for u16 and u32 (64-bit).
- `SignedBlockIndex: SignedNum` — signed in-block ptr with `Half`.
- macro `impl_num` — impls the `Num` ladder for the integer primitives.

## translator.rs

Virtual↔physical address translation, specialized by fn-pointer over the 16
(inner/outer/shift/rotation × zero/nonzero) combos so a steady param is straight-line
with no per-lookup branch. `v2p` is the hot path; `p2v` runs on remap.

Invariant: `p2v(p) = ((p + inner_offset) << shift).ror(rotation) + outer_offset`;
`v2p` is the exact inverse. `inner_offset` lives in **physical** space (added before
the shift), `outer_offset` in **virtual** space (added after). Round-trip is exact on
canonical (block-handed-out) vaddrs. **Vaddrs may wrap** (any offsetting does that);
the only hard rule is physical order: phys 0 = min element, phys len-1 = max.

- `AddressTranslator<P>` — `v2p`/`p2v`/`vdist`.
- `Translator<P>` — concrete translator holding two fn-ptrs; `set_*` re-specializes only on a zero↔nonzero flip, else just writes the field.
- `V2p`/`P2v` — fn-ptr type aliases; `variant!`/`apply!` macros generate the 16 specialized bodies.

## store.rs

Unbounded `Option<T>` slot backends (`VecStore`/`DequeStore` — cap grows on demand;
the addressable limit lives in `Mode::MAX_CAP`, not the store). Slot primitives,
spread, split, slide, find.

Invariants: `occupied ≤ len ≤ cap`; `push_*`/`grow_*`/`spread` operate on logical
slots; `find_slot`/`slide_none` honor a `pin` (kept out of the moved run).

- `Store<'a,T>` — slot-backend surface: `get`/`get_mut`/`slot`/`get_disjoint_mut`, `slide_none`, `find_slot`/`find_nearest_slot`, `push_front`/`push_back`, `grow_front`/`grow_back`/`grow`/`spread`, `insert`/`remove`, `split`, `iter`, `with_capacity`/`from_vec`/`into_vec`.
- `VecStore<T>` — Vec-backed; `DequeStore<T>` — wrap-aware (cross-slice logic for find/slide/spread/split at the deque's wrap boundary).
- `NoneSlide { from, to, delta }` — a slide; implements `Fixup` (`fix_p`: `p += delta`).
- `find_slot` — DIR-biased, budget-bounded, pin-clamped. `find_nearest_slot` — bidirectional outward variant.
- `slide_none` — rotate the run `[lo,hi]` so the `None` at `from` lands at `to`; elements shift one step toward `from`.
- `spread` — double `len`, element `i → 2i + offset` (0 = evens, 1 = odds).

## metadata.rs

The fixup protocol: block ops hand back `Fixup` implementors (grow ⇒ `GrewFixup`,
slide ⇒ `NoneSlide`); any tracked state (`BlockData`, walker data) that impls
`Fixable` receives one and corrects the addresses it holds.

- `Fixup` — `fix_p` (rewrite phys) / `fix_v` (translate, fix, translate back) / `affects_p` / `affects_v` (skip untouched pointers).
- `GrewFixup { shl, shift_offset }` — spread remap `p → p<<shl + offset`. `GrewFixup { shl: 0, shift_offset: 1 }` doubles as the plain `p → p+1` (pluripotent front-edge grow).
- `Fixable<P>` — `fixup(&mut self, f: &Fixup, tr)`. Implementors must fix every address they hold.
- `HasRoot<P>: Fixable` — block data exposing a movable root (read from `BlockData` by movable-root modes' `TreeBlock::root_position`).
- `Height`/`Depth` — pointer-free no-op-fixable meta. `Root(usize)` — phys root. `Ancestry` — stackful walker's `(parent phys, child idx)` stack, one entry per level (+`push`/`pop`/`last`/`len` helpers). `PosAncestry { pos, ancestry }` — the standard stackful walker state, satisfies the `NodeWalkerMut::State` (`Fixable` + `Clone`) contract; consumers embed it instead of reimplementing the fixup loop.

## blocks.rs

`Block` = store + translator + block data, carrying a `Mode` by type. Two surfaces:
`BlockTrait` (shared read + basic mut) and `BlockOps` (the unified per-mode
insert/split surface — a trait, not inherent methods, so the tree-ops layer can call
it generically). `Mode` owns the store type (`Mode<'block, P, N>`) and the initial
translator params. Inherent on `Block`: `new`/`from_parts`/`into_parts`/
`insert_root`/`iter`.

Invariants: `find_slot` re-translates `pos`/`pin` after a grow (vaddrs stable, phys
remap via the returned composed `GrewFixup`); `insert` fills a `None` slot and moves
no other element; every grow/slide applies its fixup to the block's **own**
`BlockData` before returning; physical order (phys 0 = min) is preserved by every op.

- `Mode<'block, P, N>` — assoc `S: Store`, consts `INNER_OFFSET`/`OUTER_OFFSET`/`SHIFT`/`INIT_CAP`/`MAX_CAP`, `make_translator`. Consts are *initial* params (vaddrs may wrap; offsets come into play at splits).
- `Uniform` — no-pin full-range (VecStore, `SHIFT = BIT_WIDTH`); for trees that grow by splitting.
- `Anchored<O>` — root pinned at `O`'s fixed vaddr (VecStore); `find_slot`/`slide_none` implicitly pin `v2p(root_vaddr)` — the root never moves.
- `Pluripotent` — sparse both-ends-growable (DequeStore, `MAX_CAP = 1 << Half::BIT_WIDTH`).
- `Block<'block, N, P, M, D, O>` — the concrete block; `UniformBlock`/`AnchoredBlock`/`PluripotentBlock` aliases.
- `BlockTrait<'block>` — assoc `N`/`P`/`S`/`BlockData: Fixable`/`O: Ordering`; `store`/`store_mut`/`translator`/`translator_mut`/`data`/`set_data`, `get`/`vget`(+`mut`)/`get_disjoint_mut`, `first_vaddr`/`last_vaddr`, `v2p`/`p2v`/`vdist`, `occupied`/`len`/`cap`, `remove`/`swap`/`swap_open`.
- `BlockOps<'block>: BlockTrait` — `find_slot(pos, after) -> FoundSlot` / `slide_none` / `insert` / `grow_and_spread` / `cleave(at)` / `cleave_and_rotate(v_start, v_end)`. Impl'd once per mode (disjoint by `M`).
- `FoundSlot { grew, slide }` — composed grow fixup + pending slide; `slide == None` ⇒ exhausted (caller must split).
- `OpenSlot(usize)` — a `None` slot opened for insert (physical).
- `compose_grew`/`grew_step`/`root_vaddr`/`fr_params` — internal helpers (grow remap composition; Anchored pin target + initial params).

Pluripotent edge insert (the unified-insert core — no element ever moves, no new
fixup type): *after-last* = `grow_back(1)` (nothing shifts); *before-first* =
`grow_front(1)` + `outer -= 1<<shift` (every existing vaddr maps to its element's new
phys, so vaddr holders stay valid; phys holders get `GrewFixup{0,1}`). Exhaustion
guard: `len < min(MAX_CAP, (1<<BIT_WIDTH) >> shift)` — past that the new vaddr range
would overlap. `find_slot` order: budgeted scan → spread + rescan → edge grow.

## walker.rs

`Node`/`SplittableNode` (the node contract: K/V/P + `DEGREE`) + the three layers.
`B` is a trait param at every level (so per-ordering gates are nameable); `O` is
never a param — it is always `B::O`.

- `Node { K, V, P, DEGREE }` — the node identity; `SplittableNode: Node` — `split(&mut self) -> Self` (drain right half).
- **Layer 1** `NodeCursor<'block,B>` — consumer stackless read: `block`/`position`(phys)/`is_leaf`/`current`/`child_count`/`child(idx)->P`/`lookup(k) -> (usize, Ordering)` (**position + comparison**: `pos` = the child `search` descends to by default; `cmp` = `k` vs that child — `Less` = before it, a new child takes slot `pos`; `Equal` = addressed by it / within its key span, slot `pos+1`; `Greater` = after it, slot `pos+1`; `(len-1, Greater)` = the append case — replaces the unreturnable `Some(len)`)/`descend`/`search` (root→terminal descent; **required, no default** — the meaning of `Equal` is a per-shape routing policy (value-storing inode stops on Eq, B+ equal-right descends), so a default would bake in one shape's semantics; the consumer interprets `lookup`'s pair). All ref returns tie to the `&self`/`&mut self` borrow (NOT `'walker` — a mut-holding cursor can't vend `'walker` shared refs; this way it reborrow safely). **No `'walker` param on the ladder** — vestigial once ref returns elided (it survived only in where-clauses, guarding nothing); the consumer's *struct* keeps its own borrow lifetime (the block stays IN the walker), but the trait bound is satisfiable at any borrow. **No constructors on the traits**: a mut-holding walker can't be built from a shared borrow, so construction lives on `From` bounds at the free fns (`impl From<&'a MyBlock> for MyCursor` — local type, orphan-safe).
- `NodeWalker<'block,B>: NodeCursor` — adds `ascend`/`depth`/`parent() -> (parent phys, child idx)`.
- `NodeWalkerMut: NodeWalker` — assoc `Payload` (what `insert_child` places in a node — node-shape-specific) + `child_payload(k, ptr)`; assoc **`State: Fixable<P> + Clone`** (the walker's fixable tracked state — position + ancestry; `Clone` is for `fixup`'s snapshot/restore; `Fixable` lives here, NOT on the walker: the block ref is never fixable) + `parts() -> (&mut State, &B)` / `parts_mut() -> (&mut State, &mut B)` (**one call returning the pair** — two separate accessors would reintroduce the state-vs-block borrow conflict; the crate's fixup path is `let (state, block) = nw.parts(); state.fixup(f, block.translator())`); `block_mut`/`current_mut`/`has_space`/`set_child(up, child_idx, ptr)` (ancestry-aware, position-stable — the fixup path rewrites a parent's entry while standing on the child)/`set_parent` (no-op for parent-free shapes)/`insert_child` (node-level wire)/`remove_child` (node-level unwire).
- **Layer 2** `TreeWalker<O, NW>` — wrapper carrying `O` as **phantom data**: it tags the Self type so the per-ordering `TreeWalk` impls (`for TreeWalker<PreOrder, NW>` etc.) sit on distinct types and pass coherence (three impls differing only in a `B::O = X` where-bound on a shared Self type hit E0119). The wrapper's `O` is bound to the block's at every use (`B: BlockTrait<O = O>`); `TreeBlock` projects it (`TreeWalker<Self::O, …>`).
- `Suggested` — the insertion-anchor plan (pure choice, no walking — `insert_child` executes it): `Parent { before }` (anchor = the current node, no walk) / `Child { idx, before }` (descend `idx`, then `subtree_first`/`subtree_last` by side).
- `TreeWalk<'block,NW,B>` — the traversal surface: `next`/`prev`/`first`/`last` (at_root + a subtree edge)/`subtree_first`/`subtree_last`/`suggest_insertion(child_idx) -> Suggested`. One impl per ordering.
- Ordering semantics: **preorder** node-first — `subtree_first` = the node itself, `subtree_last` = rightmost leaf; suggest: childless/gap 0 → after the parent, mid gap → before `child(k)` (one descend), append → after the rightmost leaf. **in-order** — both subtree edges are outermost leaves; a node sits between `child[cc/2-1]` and `child[cc/2]` (mid = cc>>1, dynamic) — a gap AT mid is the parent's own → anchor at the parent (no walk), else the adjacent child's edge leaf. **postorder** node-last — `subtree_last` = the node itself, `subtree_first` = leftmost leaf; suggest: childless → before the parent, gap 0 → before child 0's leftmost leaf, gap k → after `child(k-1)` (one descend).
- **Layer 3** `TreeWalkMut: TreeWalk` (crate impl) — `insert_child(k, node) -> Result<P, InsertErr>`: `has_space` check → `lookup(k)` → slot (`Less` ⇒ `pos`, `Equal`/`Greater` ⇒ `pos+1`; the caller guarantees a child-accepting internal — the consumer's `search` stops at terminals) → `suggest_insertion` → execute it (Parent: anchor = current position, no walk; Child: descend + subtree edge) — **the walker ends standing on the anchor** (the old boundary's phantom-phys paths are gone) → `find_slot` → apply `grew` to the walker's `State` (via `parts()`) → run-parent-fixup → `slide_none` → `State` fixup → `insert` → ascend `levels` → node-level wire. `remove_child(idx) -> (N, OpenSlot)` — unwire + slot free, no fixups. `fixup(&NoneSlide)` — run-parent-fixup **before** the slide: walk the moved run (next for delta>0, prev for delta<0; forward-only walking can't re-enter a processed subtree, so entries read on the way are unprocessed/correct); per moved node rewrite the parent's child entry via `set_child(1, idx, new_v)` (idx from ancestry — authoritative, no value scan) + repoint the node's stored parent field when the parent also moved; position-neutral via **State snapshot/restore** — a walk back would descend through just-rewritten (post-slide) child pointers over the still-pre-slide layout. The impl is **generic over `O`**: the supertrait obligation (`TreeWalker<O, NW>: TreeWalk`) is *supplied as a where-clause* rather than proven — it discharges only for a concrete `B`/`O` pair (one of the per-ordering `TreeWalk` impls), same coverage as the per-ordering impls without copying the bodies.
- `InsertErr { NodeFull, BlockExhausted }` — splits are future work; the caller handles both by splitting and retrying.
- `SplitTreeWalker` — declared sketch (`split_child`), unwired.

No `insert_child` name collision: `NodeWalkerMut` is impl'd on the consumer's `NW`,
`TreeWalkMut` on the wrapper `TreeWalker<NW>` — different `Self` types, method sets
never intersect.

## treeblock.rs

`TreeBlock` — a block whose stored type is a node. **Param-less marker trait** (the
old `C`/`W`/`WM` params existed only to feed constructor defaults; killing the defaults
killed the params, and with them the E0283 ambiguity). **Crate-impl'd for `Block` per
mode** (`impl_tree_block!` macro × `Uniform`/`Pluripotent`/`Anchored<O>`).

- `TreeBlock<'block>: BlockTrait + BlockOps` (where `Self::N: Node + Default`, `Self::BlockData: HasRoot`) — `root_position` (defaulted from `BlockData::root`).
- Free fns (construction lives here, over the consumer's `From` impls — `impl From<&'a MyBlock> for MyCursor`, local type, orphan-safe): `walker::<NW>(b)` — walker at the root, `NW: NodeWalker + From<R>`; `search::<NW>(b, k)` — walker routed to `k`'s terminal, `NW: NodeCursor + From<R>` (stackless cursors work — descent only). `R` is the borrow (`&B` or `&mut B`; both `Deref<Target = B>`), so one fn covers shared and mut — the named walker type's `From` impl picks the borrow.
- `SplitTreeBlock` — declared sketch (`split_root`), unwired, bounds at the `BlockOps` level.

## lib.rs

Module wiring + ordering markers + sketches.

- `Ordering { const ROOT_POS: RootPos }` — where the tree root lives in a fresh block; impl'd by `InOrder` (Middle)/`PreOrder` (Beginning)/`PostOrder` (End). No `TreeOrdering` marker (all orderings are tree orderings now) and no `Sorted` — deleted.
- `RelTo<T>` — `Before(T)`/`After(T)` (insert-side resolution; exported for consumers — in-crate the insertion plan is `Suggested` now).
- `FractalForest`/`BTree` — old sketches (dead).

## Testing

`src/tests/{store,block,btree}.rs` — **all currently commented out** (archived
during the refactor; `store.rs` still wires its `#[cfg(test)]` module). When the
block op surface stabilizes, adapt the store/block invariant tests to the new API.
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Status

Compiles (lib + workspace; `examples/btree.rs` — a B+ tree consumer — runs green).
Realized: the three-layer walker hierarchy (consumer node mask / `TreeWalk`
per-ordering traversal / `TreeWalkMut` tree ops); relative-position `lookup`
(`(pos, cmp)` — pos = default descent, cmp = k vs that child; append expressible as
`(len-1, Greater)`) with `search` as a **required consumer method** (no default —
the `Equal` interpretation is a per-shape routing policy); the
`Suggested`/`suggest_insertion` + `subtree_first`/`subtree_last` insertion surface
(replacing `boundary`; the walker lands on the anchor); `Mode` owns the store type;
unified `BlockOps` surface with the Pluripotent edge-grow (no block-level push);
param-less `TreeBlock` marker + `walker`/`search` free-fn constructors (E0283 gone —
no qualified-path helpers needed); `PosAncestry` standard walker state (the example
embeds it instead of hand-rolling a `Fixable` loop); fixups applied to
the block's own `BlockData` on every grow/slide; `compose_grew` for multi-grow
`find_slot` calls; preorder `prev` first-child fix (returns the parent — it
precedes its first child); fixup position-neutrality via State snapshot/restore
(replacing the walk back). The example's `two_level_demo` covers every anchor kind
plus in-run and out-of-run slides.

Not wired: **splits** (`SplitTreeWalker`/
`SplitTreeBlock` declared only), the arena tier, `leafblock`/`inline_leafblock`.

## Historical (do not revive)

`circular_array.rs`; the `MAX` const generic; `OVERP: bool` (overprovisioning is
`BlockIndex::Half`); the old signed `BlockIndex`; `AllocStrat` + `Append`/`Prepend`
modes (push-only dense blocks — superseded by the unified insert surface; pluripotent
covers them); `TreeOrdering`/`Sorted`; `O` as a generic param on walkers.

## Future Work

- splits — `split_child`/`split_root` on `SplitTreeWalker`/`SplitTreeBlock` (clone-split driver, root promotion, block cleave; see the split design below).
- extend the btree consumer (`examples/btree.rs` — currently leaf-only map inserts + a hand-assembled two-level demo) once splits land, then adapt the archived tests.
- arena tier — infallible insert (absorb exhaustion via spread/cleave/readdress), adaptive runtime strategy switching, overprovisioning, subtrees & forwarding (block_id roots), ordering across splits.
- graduation — pluripotent → concrete strategy at len == half_ptr.
- `Fixup::applies` optimization (elide unnecessary runtime checks) — deferred.
- `keys()` iter hook on the cursor (no slice — representation stays free) so B+ shapes share the child-min fetch / separator re-derivation / equal-right routing in-crate; land with or after splits (the split driver is the heaviest consumer of exactly that logic).
- ordered iteration over K/V (`IntoIterator` on the walker + consumer `is_leaf` filter; a crate-side leaf-items hook) + a range surface — deferred, but a goal.
- trie integration.

## Tree split invariants (design — in progress; predates this refactor's naming)

The block stores tree nodes in **walk (in-order) order**: physical slot order ==
in-order traversal order. This is what makes slide-fixups a sequential cursor
walk. Split insert is **bottom-up** (overflow propagates up the parent stack; may
reach the root). DEGREE ≥ 3 (a full node needs ≥2 keys to split into two
non-degenerate halves + a separator). **DEGREE is now 3.**

- **Clone before splitting an internal node (the orphan fix).** `Node::split`
  shrinks self to the left half, which orphans the right-half children — sliding to
  open the right half's median slot would move orphaned children whose inbound ptrs
  no placed node holds. **So `split_internal` clones Y first**: the original stays
  in the block, wired to its children (tree walkable, fixup's parent() works); the
  clone is split into n1/n2; after placement, `block.remove(Y)` and the parent is
  rewired to `[p1, p2]`.
- **Leaves skip the clone.** A leaf has no in-block children — nothing to orphan.
- **`target_gap(X) = phys(in_order_predecessor(X)) + 1`** — placement formula. Leaf:
  predecessor = left sibling. Internal: predecessor = `rightmost_desc(c[mid-1])`,
  `mid = child_count >> 1`. Median placement is the TARGET when a node is placed,
  not a perpetual invariant — slides preserve in-order so they can't break it.
- **vaddrs are stable across grow/spread (translator remaps) but NOT across a
  slide.** That rewrite **is** the fixup (`TreeWalkMut::fixup`). Root never in a
  slide run (pinned in Anchored).
- **In-order gap convention (current):** a node sits between `child[cc/2 - 1]` and
  `child[cc/2]` (mid = cc>>1, dynamic); gap inserts land **after** the parent (the
  parent hops later if a split demands it). Now encoded in inorder
  `suggest_insertion` (gap at mid → parent anchor); doa.md's old boundary
  special-case lines are superseded by it.

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each section afterward should include a brief overview of what the file in the codebase contains, as well as what its broad purpose is, namely what invariants it maintains. Each trait and type defined in a file should be listed, 1 or 2 lines each.
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically.
Maintain this section at the end of the claude.md .