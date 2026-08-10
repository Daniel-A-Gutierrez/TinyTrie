# doa — Dense Ordered Arenas

An alternative to malloc-per-node trees: store an ordered sequence **contiguously
in blocks, addressable by custom-width pointers** (i8..isize), as `[Option<T>]`.
Contiguous storage — even with `None` gaps — means iteration is a prefetch-friendly
linear scan and serialization is writing the bytes. The crate preserves the
**ordering** of the sequence through mutations; a contiguous run of `Some` items
may shift ±1 to make space, and gaps are reclaimed by adjusting the block's
translator. Two tiers: **Block** (a fixed-width run that surfaces address/capacity
exhaustion as `Result`/`None`) and **Arena** (automatic, adaptive, effectively
infallible — *not yet implemented*). A block pointer impls `BlockIndex`; it
semi-stably identifies an item in a block. Items may shift on neighboring inserts;
space between them is freed by growing the store and distributing items within it over its new capacity while adjusting the translator params that map virtual addresses (handed out by the block) to physical indices into the store.

## Files (lowest level → highest)

- `index.rs` — numeric trait ladder + type-level const facts underpinning all address math.
- `translator.rs` — `v2p`/`p2v` address translation, fn-ptr-specialized over zero/nonzero params.
- `store.rs` — bounded `Option<T>` slot backends + slide/find/grow/split primitives.
- `alloc_strat.rs` — per-strategy const params (a bet on a workload) + `on_grow`/`on_push_front`.
- `block.rs` — `RawBlock` (store + translator) + `BlockTrait`/`BlockMutTrait` + slide/grow/fixup.
- `block_cursor.rs` — positioned reader/writer over a block's `Some` slots (`Cursor`/`CursorMut`).
- `tree_block.rs` — `TreeBlock` (block + root vaddr + tree meta) + `TreeBlockMut`.
- `node.rs` — tree-node traits (`Node`/`INode`/`LNode`/`UnionNode`/…) + `lookup`.
- `walker.rs` — `TreeWalker`/`TreeWalkerMut` + `Probe` (stackless) / `Walker` (stackful).
- `btree.rs` — concrete B+ tree (`BINode`/`BLNode`/`BTreeMap`) over a single pre-order block.
- `lib.rs` — module wiring + ordering markers + sketches.
- stubs — `leafblock.rs`, `inline_leafblock.rs`, `abstract_tree.rs` (experiments).

## index.rs

Numeric trait ladder + type-level facts (`MIN`/`MAX`/`MIDPOINT`/`ONE`/`BIT_WIDTH`)
+ wrapping/rotate/halfptr ops, macro-impl'd for i8–i64, u8–u64. Foundation for all
address math; upholds only the numeric contract.

- `Num` — common numeric ops + const facts (`MIDPOINT` neutral anchor, `ZERO`/`ONE`/`MIN`/`MAX`/`BIT_WIDTH`) + `rotate_left`/`rotate_right`/`wrapping_*`. No `Neg`.
- `SignedNum: Num + Neg` — negation + `isize` conversion.
- `UnsignedNum: Num` — `usize` conversion (direct slot indexing).
- `BlockIndex: UnsignedNum` — unsigned in-block ptr with an associated `Half` (overprovisioning sibling). impl'd for u16 and u32 (64-bit).
- `SignedBlockIndex: SignedNum` — signed in-block ptr with `Half` (the signed seam).
- macro `impl_num` — impls the `Num` ladder for the integer primitives.

## translator.rs

Virtual↔physical address translation, specialized by fn-pointer over the 16
(inner/outer/shift/rotation × zero/nonzero) combos so a steady param is straight-line
with no per-lookup branch. `v2p` is the hot path; `p2v` runs on remap.

Invariant: `p2v(p) = ((p + inner_offset) << shift + outer_offset) rotate_left(rotation)`;
`v2p` is the exact inverse. `inner_offset` lives in **physical** space (added before
the shift), `outer_offset` in **virtual** space (added after) — the split lets a block
pin its root at the beginning/middle/end by tuning the two offsets independently
across orderings. Round-trip is exact on canonical (block-handed-out) vaddrs.

- `AddressTranslator<P>` — `v2p`/`p2v`/`vdist`.
- `Translator<P>` — concrete translator (inner/outer offsets, shift, rotation) holding two fn-ptrs; `set_*` re-specializes only on a zero↔nonzero flip, else just writes the field.
- `V2p`/`P2v` — fn-ptr type aliases; `variant!`/`apply!` macros generate the 16 specialized bodies.

## store.rs

Bounded `Option<T>` slot backends, `MAX_CAP` const (pow2, const-asserted). Slot
primitives, spread, split, slide, find_slot, iter/cursor.

Invariants: `occupied ≤ len ≤ cap ≤ MAX_CAP`; `len` is a power of two; `push_*`/`grow_*`/`spread` assert against `MAX_CAP`; capacity doubles up to `MAX_CAP`. `find_slot`/`slide_none` honor a `pin` (kept out of the moved run).

- `Store<'a,T>` — slot-backend surface: `MAX_CAP`, `occupied`/`len`/`cap`, `push_back`/`push_front`, `insert`/`remove` (keep `len`, change `occupied` ±1), `grow_*`, `spread`, `split`/`split_and_rotate`, `slide_none`, `find_slot`/`find_nearest_slot`, `iter`/`cursor`, `slot`/`get`/`get_mut`.
- `VecStore<T,CAP>` — Vec-backed; `DequeStore<T,CAP>` — wrap-aware (adds cross-slice logic for `find_slot`/`slide_none`/`spread`/`split` at the deque's wrap boundary).
- `NoneSlide { from, to, delta }` — a slide; `fix_p` rotates a phys by the slide's delta.
- `find_slot` — DIR-biased, budget-bounded, pin-clamped (pin<pos raises min, pin>pos lowers max, pin==pos restricts to the DIR side). `find_nearest_slot` — bidirectional outward variant.
- `slide_none` — rotate the run `[lo,hi]` so the `None` at `from` lands at `to`; elements shift one toward `from`. `from==to` is a no-op.
- `spread` — double `len`, element `i → 2i+SPREAD_OFFSET`. `split(at)` / `split_and_rotate(at)` — partition halves (the latter odds-gaps both + bumps rotation).

## alloc_strat.rs

Per-strategy const params — the bet on a workload — plus `on_grow`/`on_push_front`.
A strategy optimizes one insert pattern and loses on another.

- `AllocStrat<P>` — consts: `INIT_SHIFT`, `INIT_CAP`, `INIT_INNER_OFFSET`, `INIT_OUTER_OFFSET`, `SPREAD_OFFSET`, `INNER_OFFSET_GROWS`, `OUTER_OFFSET_SHRINKS`, `INSERT_BUDGET`, `CAP_LIMIT`, `REVERSED`, `INIT_ROOT`; fns `on_grow`/`on_push_front`/`on_push_back`.
- `Uniform<O>` — random-optimized: full-range shift, anchor `MIDPOINT`, `VecStore`; `try_insert_*` always `Err` (random-only via `find_slot`); auto-spreads at `occupied*3 > len*4`.
- `Pluripotent<O>` — "don't know the workload yet": `Half`-range, `DequeStore`; `on_push_front` lowers **outer** (not inner) to keep slots canonical at `shift>0`.
- `Append` — hot `push_back` dense; cold `push_front` into a reserved low range; `on_push_front` does `inner -= 1` (shift 0).
- `Prepend` — `Append` mirrored (`REVERSED`): hot `push_front` = physical `push_back`, iteration high→low.
- `strat!` / `strat_push_front!` macros — emit one `AllocStrat` impl per row.

## block.rs

`RawBlock` = a `Store` + a `Translator`, carrying an `AllocStrat` by type. Upholds
**no** structural/tree invariant — only the address-model invariants. `BlockTrait`/
`BlockMutTrait` are the read/mutation surfaces.

Invariants (address-model only): `find_slot` re-translates `pos`/`pin` after a grow
(spread shifts phys `i→2i`; vaddrs stable); `insert` fills a `None` slot and moves no
other element; a `grew` fixup is returned for the caller to apply to live phys values.

- `FoundSlot { grew, slide }` — `find_slot` result: an optional `GrewFixup` (apply to live phys) + an optional pending `NoneSlide` (apply via `slide_none`).
- `GrewFixup { shl, shift_offset }` — spread remap `p → p<<shl + shift_offset`.
- `OpenSlot(usize)` — a `None` slot opened for insert.
- `BlockTrait<'a>` — read surface: assoc `T`/`P`/`S`/`Cursor`; `store`/`translator`/`get`/`vget`/`first_vaddr`/`last_vaddr`/`v2p`/`p2v`/`occupied`/`len`/`cap`/`max_capacity`/`iter`/`cursor`.
- `BlockMutTrait<'a>: BlockTrait` — mutation: assoc `A: AllocStrat`; `store_mut`/`translator_mut`/`cursor_mut`/`get_mut`/`vget_mut`/`slide_none`/`insert_root`/`grow_and_spread`/`find_slot`/`insert`/`remove`/`swap`/`swap_open`/`try_insert_back`/`try_insert_front`/`split`/`split_and_rotate`.
- `RawBlock<'a,T,P,A,S>` — store + translator + strategy (`PhantomData`). Per-strategy `BlockMutTrait` impls for the four (strategy, store) combos.
- `DirIter<F,R>` — fwd/rev iter wrapper (REVERSED picks `Rev`). `SlotDebug<P>` — debug rendering aid.

## block_cursor.rs

A positioned reader/writer over a block's `Some` slots; tracks a **physical** slot
internally. `Cursor`/`CursorMut` traits; `BlockCursor` impls both. Mutation helpers
(`find_slot`/`slide_none`/`insert`/`remove`/`swap`) apply the block's fixups to the
tracked element so it survives grow/slide.

Invariants: the tracked phys stays valid across grow/slide (the cursor self-fixes);
`seek` takes **phys** with no bounds/occupancy assert (`store.get` panics on `None`/OOB
— the store's responsibility); `vseek` is a vaddr convenience (default over `seek`+`v2p`);
`into_parts` returns **phys** (no premature vaddr translation); `from_parts` takes a
**vaddr** (the stable-across-mutation handle).

- `Cursor<'cursor,T,P>` — positioned read: `address` (vaddr) / `position` (phys) / `current` / `seek`(phys) / `vseek`(vaddr, default) / `next` / `prev` / `first` / `last` / `p2v` / `v2p`.
- `CursorMut<'cursor,T,P>: Cursor` — adds `current_mut`.
- `BlockCursor<'block,'cursor,B,R>` — block-backed cursor (`R = &B` shared, `&mut B` mut); inherent: `new`/`new_at`/`into_parts`(phys)/`from_parts`(vaddr)/`p2v`/`v2p`/`slot_occupied`/`root_phys`; mut: `find_slot`/`slide_none`/`insert`/`remove`/`swap`/`swap_open`.

## tree_block.rs

`TreeBlock` = a `RawBlock` + a root vaddr + tree meta; `TreeBlockMut` adds meta/root
access. Delegates block ops to the inner `RawBlock`.

Invariants: the root vaddr is stable (the translator remaps); meta holds tree-level
state (e.g. `Height`).

- `TreeBlock<'a,T,P,A,S,O,Meta>` — block + `root: P` + `meta: Meta` (+ ordering phantom).
- `TreeBlockMut<'a>` (trait, on top of `BlockMutTrait`) — `meta`/`set_meta`/`root`/`set_root`; assoc `Meta`/`K`/`V`/`O`.
- `BlockTrait`/`BlockMutTrait` impls delegate to the inner `RawBlock` (incl. REVERSED-aware `iter`).

## node.rs

Tree-node traits — the abstraction a tree (the btree) is built over. `Node`
(K/V/P/DEGREE), `INode`/`LNode` (routing/leaf storage), `UnionNode` (untagged
inode|lnode, variant external via height), `HasParent` (a kind-free parent field).
`lookup` returns maximal `(pos, Ordering)` so a single scan feeds get/remove/insert/route.

Invariants: `UnionNode` is a bare `union` (`Copy`); variant is known externally
(height-discriminated), not stored on the node; parent is hoisted kind-free on the
wrapper so a stackless walker can fix a moved node's parent without an ancestor stack.

- `D` / `DoubleExact` — shorthand bounds (`D = 'static+Sized`; `DoubleExact = DoubleEnded+ExactSizeIterator`).
- `Node` — `type K/V/P`, `const DEGREE`.
- `HasParent<P>` — `parent`/`set_parent` (kind-free, on the wrapper not the variants).
- `OrphanUnionNode<I,L>` — bare untagged union `inode|lnode`; never `HasParent`.
- `UnionNode<I,L>` — `OrphanUnionNode` + hoisted `parent: I::P` (the stackless-fixup enabler).
- `EnumNode<I,L>` / `EnumRef<P>` / `TaggedChildNode` — tagged variants (sketches).
- `LNode<K,V>: Node` — leaf: `values`/`pairs`/`keys`/`lookup`/`insert`/`insert_at`/`remove`. `lookup(k) = (pos, cmp)`: `pos` = first key ≥ k, `cmp = k.cmp(keys[pos])` or `Greater` past end; `Equal` ⇒ hit at `pos`. `insert_at(pos,…)` inserts without rescanning (the B+ insert path).
- `ValueNode<V>` — single-value node (sketch).
- `SplittableNode<K>: Default` — `split_into(&mut blank) -> K`: drain the right half into `blank`, return the separator.
- `INode: Node` — routing: `keys`/`lookup`/`child`/`children`/`insert_child`/`remove_child`. `lookup(k) -> Option<(pos,cmp)>`: same partition as `LNode`; `None` ⇒ stop here (terminal / in-node match), `Some` ⇒ descend (B+ inodes always `Some`, so the walker may `unwrap_unchecked`).
- `IVValue` — internal node with values (sketch).

## walker.rs

Tree-walker traits + `Probe` (stackless, no alloc) / `Walker` (stackful, with stack).
`TreeWalker` is generic over `C: Cursor` — one impl serves both the shared (`&B`) and
mut (`&mut B`) `Probe` variants, since navigation touches only `Cursor` methods and
`CursorMut: Cursor`. `TreeWalkerMut: TreeWalker` inherits navigation and adds mutation.
Construction (`new`/`new_mut`) is cursor-specific, so it is inherent on `Probe`,
node-agnostic.

Invariants: `current_into` consumes the walker and returns the cursor — ref extraction
(`into_parts` + `get`) stays with the consumer, where the cursor is concrete and
lifetimes are provable. `fixup` runs **before** the slide (vaddrs stable then) and is
position-neutral (saves/restores the cursor).

- `Height(pub u64)` — `TreeBlock` meta for fixed-height trees (B+); `Copy`.
- `Probe<'block,B,C,M>` — stackless walker: `cursor` + `depth` + `meta`. `new`/`new_mut` inherent, generic over `B`/`M` (know nothing of the node type).
- `Walker<'block,B,C>` — stackful walker (`stack: Vec<(P,usize)>`); sketch.
- `TreeWalker<'block,'walker,O,B,C>` (`C: Cursor`) — `go_next`/`go_prev`/`descend`/`descend_right`/`descend_left`/`ascend`/`depth`/`position`/`walk_to`/`current_into(self) -> C`.
- `TreeWalkerMut<…>: TreeWalker` (`C: CursorMut`) — `current_mut`/`insert_child`/`remove_child`/`split_child`/`swap_none`/`fixup`/`fixup_stack`.
- `remove_child(child) -> (B::T, OpenSlot)` — unwires child[idx] + its bounding separator AND frees the child's block slot (a `Some` orphan is a fixup landmine — `fixup` would process it on a later slide and panic). Returns the removed node **by value** + the freed `OpenSlot`; the merge driver moves the node's contents into the kept node from the returned value (after, not before — the slot is already `None`). Removes a **leaf** child only (cannot remove inodes); an inode with no keys is left as-is for now (TODO).
- `fixup(&NoneSlide)` — run-parent-fixup before a slide: rewrite each moved node's stale parent→child pointer (and its hoisted `parent` if the parent also moved). Probe impl vseeks around the run (O(DEGREE) child-scan — the cost of stackless); a stackful walker over traversal-ordered storage would walk the run and read parents off its stack (O(1)). `fixup_stack` is the older list-driven variant.

## btree.rs

A concrete B+ tree (`BNode = UnionNode<BINode, BLNode>`) over a single pre-order
`Uniform` block. `BTreeMap` routes by key to the leaf and edits in place. B+
min-separator convention: `keys[i] = min(child[i+1])`; `#keys = #children - 1`.

Invariants: the block stores nodes in **pre-order** (physical slot order == in-order
traversal order — makes slide-fixup a sequential cursor walk); height discriminates
inode (`depth < height`) from lnode (`depth == height`); `new()` inserts the root leaf
so `insert` never checks for an empty block; `remove` only edits leaf keys (never frees
the root block slot, so `block.len()` stays ≥ 1).

- `trait C: D + Copy` (alias) — `K: C`, `V: C`. The `Copy` is forced by the bare-union node design (revisit; see Future Work).
- `BINode<K,V,P>` — internal: `keys: TinyArray<K,15>`, `children: TinyArray<P,16>`. `INode::lookup` (first ≥ k, always `Some`); `insert_child`/`remove_child` (B+ separator placement).
- `BLNode<K,V,P>` — leaf: `keys`/`values: TinyArray<_,15>`. `LNode::lookup`/`insert`/`insert_at`/`remove`.
- `BNode<K,V,P> = UnionNode<BINode,BLNode>` — the stored node; `Default` = empty leaf (inode field uninit).
- `SplittableNode` impls for `BINode`/`BLNode` (`split_into`).
- `node_full` — variant-dependent fullness (inode: `children.len()==DEGREE`; leaf: `keys.len()==LEAF_MAX`).
- generic `TreeWalker`/`TreeWalkerMut` impls (BNode-specific, generic over the cursor `Cs`): `walk_to` (`lookup` + B+ routing `pos + (cmp==Equal)`, `unwrap_unchecked`), `descend*`/`ascend`, `insert_child` (anchor → `find_slot` → `fixup` → `slide_none` → place → wire parent), `remove_child` (atomic free), `fixup` (the run-parent-fixup loop, position-neutral).
- `BTreeMap<K,V,P=u16,CAP=4096>` — the public map: `new` (inits root), `get`/`get_mut`/`insert`/`remove` via `walk_to` → `current_into` → `into_parts`(phys) → `block.get`/`get_mut` → `leaf.lookup`. `insert` uses `lookup`+`insert_at` (one scan); panics on a full leaf (split not wired).

## lib.rs

Module wiring + ordering markers + sketches.

- `Ordering` trait + `InOrder`/`PreOrder`/`PostOrder`/`BFO` markers.
- `RelTo<T>`, `BPtr`/`IPtr`/`LPtr` aliases, `Fixup` trait (vaddr/phys fixup via a translator).
- `FractalForest`/`BTree`/`INode` sketches (unused).

## stubs — `leafblock.rs` / `inline_leafblock.rs` / `abstract_tree.rs`

- `leafblock.rs` — `SlicePtr`/`PtrUnion`/`LeafBlock`/`GrowErr`: a *random* leafblock — leaves are slices scattered across the address space with `None` gaps, grow by claiming adjacent gaps, reorg via `split_and_rotate` (pointer-rotation, no full readdress). Sketch.
- `inline_leafblock.rs` — `Mode`/`Sparse`/`Dense`/`Header`/`UData`/`EData`/`LeafBlock`: leafnode headers stored inline alongside their keys/values (a vec of inline vecs). Sketch.
- `abstract_tree.rs` — `TreeNode<IDX,PTR>` trait, 1-line stub.

## Testing

Tests live in `src/tests/{store,block,btree}.rs`, pulled in via `#[cfg(test)] #[path = "tests/…"]`
at the end of each source file (in-crate, so `pub(crate)` internals are visible). They
encode the address-model and per-strategy invariants; a reference-comparison harness
for the store's `find_*`/`slide_none` over all `(from,to)`/`pos×dir×budget×pin`, contiguous
and wrapped; per-strategy block hot/cold paths, vaddr stability across grow/spread,
pin-root-never-moves, exhaustion; btree map ops.

⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory — run
targeted tests, not the whole suite.

## Status

Compiles. Realized: the index/translator/store/alloc_strat/block/block_cursor/tree_block/node/walker/btree
tiers; `lookup`+`insert_at` (one-scan leaf ops, no rescans); `fixup` extracted to a
per-walker trait method; `remove_child` made atomic (unwire + free the slot, no orphan
landmine); `new()` inits the root (insert drops the empty-guard); node-agnostic
constructors on `Probe`; `into_parts` returns phys (no premature vaddr translation).

Not wired: **splits** (`insert` panics on a full leaf), **merges**/underflow handling
(`remove` does key-removal only — no borrow/merge), the **arena tier** (auto-split,
adaptive strategy switching, infallible insert), `leafblock`/`inline_leafblock`/`abstract_tree`.
`V: Copy` (forced by the bare-union node) is a deliberate constraint to revisit.

Historical (do not revive): `circular_array.rs`; the `MAX` const generic; the `OVERP: bool`
block generic (overprovisioning is now `BlockIndex::Half`); `BlockIndex::sqrt_max`; the
old signed `BlockIndex` (split into unsigned `BlockIndex` + `SignedBlockIndex`).

## Future Work

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

## Tree split invariants (design — in progress)

The block stores tree nodes in **walk (in-order) order**: physical slot order ==
in-order traversal order. This is what makes slide-fixups a sequential cursor
walk. Split insert is **bottom-up** (overflow propagates up the parent stack; may
reach the root — the old proactive ≤1-level guarantee is retired; bottom-up splits
only when actually needed, for better space utilization). DEGREE ≥ 3 (a full node
needs ≥2 keys to split into two non-degenerate halves + a separator; DEGREE=2 can't
split). **DEGREE is now 3.**

- **Clone before splitting an internal node (the orphan fix).** `Node::split`
  (`split_off`) shrinks self to the left half, which orphans the right-half
  children — they're in the block but no placed node references them (inbound ptrs
  only in the owned right half). Sliding to open the right half's median slot would
  move orphaned children and `fixup_moved_run` couldn't update their inbound ptrs
  (no placed parent). **So `split_internal` clones Y first**: the original Y stays
  in the block, intact, wired to its children (tree walkable, fixup's `parent()`
  works); the clone is split into n1/n2 (the only new floating nodes); after
  `insert_2` places them, `block.remove(Y)` frees Y's slot and the parent is
  rewired to `[p1, p2]`. Y's children stay referenced throughout (by Y, then n1/n2).
- **Leaves skip the clone.** A leaf (terminal, height `MIN`) has no in-block
  children (only external `SlicePtr`s) — nothing to orphan. `split_leaf` does
  `L.split()` in place (L becomes n1), no clone, no remove.
- **`target_gap(X) = phys(in_order_predecessor(X)) + 1`** — placement formula. Leaf:
  predecessor = left sibling. Internal: predecessor = `rightmost_desc(c[mid-1])`,
  `mid = child_count >> 1`. Median placement is an ORDERING property (node between
  its two median children's subtrees), not an exact address — slides preserve
  in-order so they can't break it. The median rule is the TARGET when a node is
  placed, not a perpetual invariant on every node.
- **`Node::split(&mut self) -> (Self, K)`** — logical only. Drains the right half
  into a new node; self keeps the left half; returns `(right, separator)`. No phys,
  no fixup. Used on clones (internal) and on L directly (leaf).
- **`Node: Default`** — `default()` is the empty node (INode: `empty()`); used by
  `split_internal`'s root branch to construct the new root.
- **`Payload<V,P>` / `Overflow { right, sep }`** — `Payload` is a tagged enum (not
  two `Option`s) so exactly one payload is expressible. `Node::insert` maps
  `Payload`→storage; `Overflow` carries a whole node, not a payload. (`Node::insert`
  is NOT used on the split path — the driver uses `insert_bucket` +
  `split_leaf`/`split_internal`; it stays as a trait method for consumers.)
- **`insert_2`** — two sequential placements (no combined slide): place n1
  (floating = `[child_in]`), then place n2 (floating = `[n1, child_in]`). Each
  placement = subtree-aware anchor (`target_gap`) → `find_slot` (pin=root) →
  `fixup_moved_run` → `slide_none` → `insert`. One slide may pass over n1 or
  `child_in` (placed-but-unwired) → `fixup_moved_run`'s floating branch updates the
  handle. The walker must be at the anchor with a valid ancestor stack before each
  `fixup_moved_run`.
- **`split_leaf(L)`** — ask `parent.insert_position(parent_v, L_idx+1)` (anchor
  resolves to `After(L)` since n2 is terminal); `L.split()` (L=n1 in place);
  single-insert n2 after L; return `(sep, n2_v)` to the driver.
- **`split_internal(Y, sep_in, child_in)`** — `clone=Y.clone(); (n2,sep)=clone.split();`
  route `(sep_in, child_in)` into n1 or n2; `insert_2` places n1, n2;
  `block.remove(Y)`; rewire: **root** → new root (`Node::default()`) at Y's freed
  slot (= `tree.root`, inv 2) over `[p1, p2]` + sep, bump height, return None;
  **non-root** → `grandparent.children[Y_idx] = p1`, return `(sep, p2)` for the
  driver. (p1-replacement is child-count-neutral; the +1 is the returned `(sep,
  p2)` the driver inserts into the grandparent.)
- **`Walker::insert` (driver)** — descend to leaf (no pre-split); leaf has room →
  `insert_bucket`; else `ov = split_leaf(L)`. Loop: `ov=Some((sep,child))` → pop
  parent; parent has room → `parent.insert(sep, Payload::Child(child))`, done;
  else `ov2 = split_internal(parent, sep, child)` (`None` ⇒ parent was root, new
  root placed, done; `Some(ov)` ⇒ propagate to grandparent).
- **vaddrs are stable across grow/spread (translator remaps) but NOT across a
  slide.** A slide shifts ≤`bit_width` elements ±1; their phys changes ⇒ vaddr
  changes ⇒ the parent holding that child's vaddr has a stale pointer. That
  rewrite **is** the fixup. Root is never in a slide run (pinned, inv 4).
- **`fixup_moved_run(slide, &mut [P] floating)`** — DONE. Cursor from the slide's
  insertion point in the slide direction; per moved node, `old_v = p2v(anchor_p −
  delta)`, if `old_v` in `floating` → rewrite that handle entry to the new vaddr;
  else rewrite the child pointer in its parent (`parent.children[j] = new_v`,
  parent from the cursor's ancestor stack). The run is contiguous in-order, so
  `next()`/`prev()` + the ancestor stack enumerate it. The `anchor_moves` check
  pre-steps the cursor when the anchor is stationary.
- **Anchor may or may not be in the run.** `find_slot` prefers a `None` on the
  insert side; if found, the slide is between anchor and that `None` and the
  anchor doesn't move. Else the anchor is in the run and is fixed up like any
  other node. `fixup_moved_run` handles both uniformly (the `anchor_moves` branch).

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each section afterward should include a brief overview of what the file in the codebase contains, as well as what its broad purpose is, namely what invariants it maintains. Each trait and type defined in a file should be listed, 1 or 2 lines each. 
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically. 
Maintain this section at the end of the claude.md . 