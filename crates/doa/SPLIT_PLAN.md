# doa — bottom-up tree split (plan)

Read `crates/doa/CLAUDE.md` first (esp. "Tree split invariants"). Source:
`src/tree_traits.rs` (`Node`/`Walker`/`TreeRoute`/`TreeNav`, `Payload`/`Overflow`,
`Node::split`/`Node::insert` (done), `Walker::fixup_moved_run` (done),
`Walker::insert_2`/`split_leaf`/`split_internal`/`insert` — to fill; legacy
`insert_child`/`insert_root` fixup), `src/lib.rs` (`INode`, `split_off`/`insert_bucket`,
`Default`, legacy `split_child`/`split_root`/`UBTree::insert`), `src/block.rs`
(`find_slot`, `slide_none`, `insert`, `remove`, `swap`), `src/store.rs` (`NoneSlide`).

## Invariants (do not violate)

1. **No sentinel values.** `P::MIN` (0 unsigned) is a valid handed-out vaddr.
   `INode::empty`/`Default` uses `I::MIN` as a node-field null convention, NOT an
   address reservation.
2. **Root at its traversal position, invariant under promotion.** InOrder → phys
   `len/2` (vaddr `MIDPOINT`). `tree.root` does NOT change when a new root is
   promoted — the new root ADOPTS the root vaddr (placed at the freed old-root
   slot). Spread doubles len so root phys moves `1→2→4` (= `len/2`); the vaddr is
   the stable thing.
3. **Physical slot order == traversal (walk) order.** THE invariant the slide-fixup
   relies on. The moved physical run is a contiguous in-order run, so `next()`/
   `prev()` enumerate it and `parent()` yields each moved node's parent.
4. **Root pinned during placement.** `find_slot`/`slide_none` pin-clamped to
   `self.root()`; a slide never covers the root. The root is freed only by
   `split_internal`'s root branch (`block.remove`), never by a child's slide.
5. **Bottom-up split; split-on-overflow.** A node splits only when an insert
   overflows it (full + gains a key/child). Propagation goes up the parent stack;
   can reach the root. The old proactive top-down ≤1-level guarantee is RETIRED —
   bottom-up splits only when actually needed (better space utilization). DEGREE
   ≥ 3 (a full node needs ≥2 keys to split into two non-degenerate halves + a
   separator; DEGREE=2 can't split). **DEGREE is now 3.**
6. **The tree must be walkable when a fixup runs.** `fixup_moved_run` cursor-walks
   the tree (`next()`/`prev()` + the ancestor stack), so every WIRED node must be
   linked when it runs. The only disconnected nodes allowed are FLOATING nodes
   (placed but not yet wired into their parent) — handled via handles, not
   traversal.

## The clone mechanism (resolves the orphaned-children problem)

`Node::split` (`split_off`) shrinks self to the left half, which ORPHANS the
right-half children: they're in the block but no placed node references them
(their only inbound ptrs are in the owned right half, not in the block). If we
then slide to open the right half's median slot, the slide moves those orphaned
children and `fixup_moved_run` can't update their inbound ptrs (no placed parent)
→ mis-fire `parent()` on a wired node → corruption.

**Solution: CLONE the internal node before splitting.** The original X stays in
the block, intact, still wired to all its children — so the tree stays walkable
and `fixup_moved_run`'s `parent()` works on the moved (wired) nodes. The clone is
split (clone becomes n1, returns (n2, sep)); n1 and n2 are the only NEW floating
nodes. After `insert_2` places them (X still intact), `block.remove(X)` frees X's
slot, and the parent is rewired to `[p1, p2]`. X's children stay referenced
throughout (by X, then by n1/n2).

**LEAF nodes (terminal, height MIN) have no in-block children** (only external
`SlicePtr`s), so there's nothing to orphan — `split_leaf` skips the clone:
`L.split()` makes L into n1 in place, no remove.

## Two split routines (dispatched by height)

### `split_leaf(L)` — L terminal (height MIN)
- ask `parent.insert_position(parent_v, L_idx+1)` for the anchor; `n2` is
  terminal so its subtree extremity is itself → anchor resolves to `After(L)`.
- `let (n2, sep) = L.split();` — L becomes n1 in place (left half). No clone, no
  remove.
- place `n2` after L (single insert: `find_slot(anchor, After, pin=root)` →
  `fixup_moved_run(slide, &mut [])` → `slide_none` → `insert`). L stays wired so
  the fixup's `parent()` works on moved siblings.
- return `(sep, n2_v)` for the driver to insert into the parent.

### `split_internal(Y, sep_in, child_in)` — Y internal (height > MIN)
- `clone = Y.clone();`
- `(n2, sep) = clone.split();` — clone = n1 (left half). Y stays in the block,
  intact, wired to its children.
- route `(sep_in, child_in)` into n1 or n2 (whichever key range contains
  `sep_in`). both halves have room (DEGREE≥3).
- `insert_2((anchor1,dir1), n1, (anchor2,dir2), n2)`:
  anchors = `target_gap` (`rightmost_desc(c[mid-1])`, After). place n1 (floating =
  `[child_in_v]`), then place n2 (floating = `[n1_v, child_in_v]`). one slide may
  pass over n1 or child_in → its handle updates (floating branch of
  `fixup_moved_run`).
- `block.remove(Y);` — free Y's slot (block primitive, NOT walker `remove`).
- rewire:
  - **root** (Y had no parent): place new root (`Node::default()`) at Y's freed
    slot (= `tree.root`, inv 2) over `[p1, p2]` + `sep`, bump height; return None.
  - **non-root**: `grandparent.children[Y_idx] = p1`; return `(sep, p2)` for the
    driver to insert into the grandparent.

### `insert_2` — two sequential placements
place n1 (floating = `[child_in_v]`), then place n2 (floating = `[n1_v,
child_in_v]`). each placement = `find_slot` + `fixup_moved_run` + `slide_none` +
`insert`. the second placement's slide may pass over n1 (placed-but-unwired) or
`child_in` (placed-but-unwired) → `fixup_moved_run` updates those handles
(floating branch) instead of a parent. NO combined slide; reuse
`fixup_moved_run` as-is.

**Only the root is pinned (inv 4).** `pin = self.root()` for both placements. Y is NOT
pinned, so it may move in placement 1's slide; `insert_2` **re-descends from the (pinned,
stable) root** between placements (`reposition_to_anchor`: reset to root via
`set_position`+`set_height`+clear-stack, descend the root→Y `path`, then
`descend_to_rightmost_desc(child_idx)`). The root→Y path stays live because
`fixup_moved_run` keeps every moved node's parent pointer current. `path` (child indices
root→Y) is passed by the caller (`split_internal`).

### `target_gap(X) = phys(in_order_predecessor(X)) + 1`
- leaf: predecessor = left sibling.
- internal: predecessor = `rightmost_desc(c[mid-1])`, `mid = child_count >> 1`.

Median placement is an ORDERING property (node sits between its two median
children's subtrees), not an exact address — slides preserve in-order so they
can't break it. The median rule is the TARGET when a node is placed, not a
perpetual invariant on every node.

### Driver (bottom-up)
```
descend to leaf L (no pre-split); stack = path
if L has room: L.insert_bucket(k, v); return
ov = split_leaf(L)                                  // (sep, n2_v)
while let Some((parent_v, Y_idx)) = stack.pop():
    parent = block.get(parent_v)
    if parent has room:
        parent.insert(sep, Payload::Child(child)); return
    else:
        ov2 = split_internal(parent, sep, child)     // Y = parent
        match ov2:
            None    => return                        // parent was root → new root placed
            Some(ov) => { (sep, child) = ov }         // propagate to grandparent
```
`split_internal` does the p1-replacement (`grandparent.children[Y_idx]=p1`,
child-count-neutral) internally and returns `(sep, p2)` — the +1 that may
overflow the grandparent (handled by the next loop iteration).

## Status

- **Foundation GREEN.** Translator `inner_offset`/`outer_offset` split;
  `alloc_strat.rs` remapped (Pluripotent hand-written `outer=MIDPOINT` +
  `on_push_front outer -= 1<<shift`; Append/Prepend `inner=1<<Half` +
  `on_push_front inner -= 1`; Uniform\<InOrder\> `shift=width-1, inner=0`,
  root p2v(1)=MIDPOINT). 74 block/store tests pass. Tree path: u32 root vaddr
  `1<<31`.
- **Step 2 DONE.** `TreePos::height()` + `TreeRoute` defaults
  `leftmost_desc`/`rightmost_desc`/`extremity_at`. Test `leftmost_rightmost_desc`
  passes.
- **DEGREE 2→3 DONE.** `INode::DEGREE=3`; `keys[K;2]`, `leaves[PtrUnion;3]`,
  `children_array`'s `[Option<I>;3]` + const assert; test helpers `inode`/`mk_inode`
  resized. `cargo check --lib` 0 errors.
- **`Node::split`/`Node::insert` DONE.** `split` wraps `split_off` (tuple-swap to
  `(right, sep)`); `insert` maps `Payload`→`PtrUnion`, split-when-full, routes
  into the owning half, returns `Overflow`. (`Node::insert` is NOT used on the
  split path — the driver uses `insert_bucket` + `split_leaf`/`split_internal`;
  `Node::insert` stays as a trait method, may be used by consumers.)
- **`fixup_moved_run` DONE.** Cursor-walks the moved in-order run; per moved node,
  `old_v = p2v(anchor_p - delta)`, if in `floating` → update handle, else
  `parent().update_child(ci, new_v)`. Root never in a run (inv 4). Anchor-may-
  -or-may-not-move handled by the `anchor_moves` pre-step. 0 errors.

## Dead end — do not repeat

A previous attempt deleted the fixup and placed each new node at the nearest
DIR-side `None` WITHOUT sliding (`OpenSlot(slide.from)`), reasoning "key routing
works regardless of physical order." This violates inv 3 (linear-scan iteration
/ serialization needs physical==in-order) AND fails `ubtree_insert_many`
(no-slide ⇒ `grow_and_spread` finds a DIR-side None each split ⇒ `len` doubles ⇒
`UB_CAP` exhaustion). It also rewrote `remove` and read `debug_height` in logic
(that field is debug-only by contract). Reverted. The fix MUST slide + fixup over
a contiguous in-order run.

## Roadmap (remaining)

1. **`insert_2`** — two sequential placements with cross floating handles
   (`[child_in]` then `[n1, child_in]`). Reuses `fixup_moved_run`. The placement
   = subtree-aware anchor (`target_gap`) → `find_slot` (pin=root) →
   `fixup_moved_run` → `slide_none` → `insert`. Cursor-positioning at the anchor
   is the caller's setup (the walker must be at the anchor with a valid ancestor
   stack before `fixup_moved_run`).
2. **`split_leaf`** — terminal split utility (no clone, no remove). Ask
   `parent.insert_position`, `L.split()`, single-insert n2, return
   `(sep, n2_v)`.
3. **`split_internal`** — clone + `Node::split` on clone + route incoming +
   `insert_2` + `block.remove(Y)` + rewire (root branch / non-root return
   `(sep, p2)`).
4. **`Walker::insert` driver + rewire `UBTree::insert` + delete legacy
   `split_child`/`split_root`.**
5. **Un-ignore `ubtree_insert_many`** + final `cargo check --lib`.

ORDER: 1 → 2 → 3 → 4 → 5. the tree must be intact when any fixup runs (inv 6), so
each placement's fixup completes before the next placement/wiring.

## Test bar

- `ubtree_single_node` passes fully (insert/get/remove/range, 5 keys). [currently
  panics — legacy `UBTree::insert` still wired]
- `ubtree_root_split_only`, `ubtree_get_mut`, `probe_maps_key_to_lptr`,
  `debug_layout_demo`, `leftmost_rightmost_desc` stay green.
- `ubtree_insert_many` un-ignored, passing (300 inserts/get/remove/re-get).

⚠️ **Don't run the full `cargo test`** — it OOMs the IDE. Use `cargo check --lib`
only, or a single named test with the user's OK.

## Tracing notes

- Tests print the block `Debug` layout (`i:[child_phys,...]`, `j:X`) per insert.
  When a test fails, read it and CHECK inv 3 by hand: map each node's child vaddrs
  to phys, write the in-order sequence, compare to physical slot order.
- vaddr↔phys depends on the current translator; spreads halve `shift` and remap
  `i→2i`, vaddrs stable across spreads (re-translate pos/pin after —
  `block.rs::find_slot`).
- `eprintln!` `[fixup]`/`[next]` lines are debug instrumentation; remove once green.
- DEGREE=3 (small for tracing). `INode::debug_height` is debug-only — never read
  it in logic.