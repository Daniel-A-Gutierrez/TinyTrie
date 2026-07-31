# doa — subtree-aware placement & block-level split (plan)

Picks up from a debugging session on `tree_traits::tests::ubtree_single_node`.
Read `crates/doa/CLAUDE.md` first (architecture, address model, testing). The
relevant source: `src/tree_traits.rs` (Walker/Probe/TreeBlock, `insert_child`,
`insert_root`), `src/lib.rs` (`UBTree`, `INode`, `split_root`, `split_child`,
`insert`), `src/block.rs` (`RawBlock`, `find_slot`, `slide_none`, block
`insert_root`), `src/store.rs` (`NoneSlide`, `find_slot`, `slide_none`).

## Invariants we are upholding (do not violate these)

1. **No sentinel values.** `P::MIN` (0 for unsigned) is a *valid* handed-out
   vaddr. Never treat 0 as null/reserved/exhausted in the address model.
   (`INode::empty` initializes child ptrs to `I::MIN` as an INode-level null
   convention — that is a node-field convention, NOT an address reservation.
   Don't confuse the two.) A node legitimately living at vaddr 0 is fine.

2. **Root at its traversal-order position.** The root's physical slot is its
   position in the chosen ordering: InOrder → `MIDPOINT` (phys `len/2`), PreOrder
   → phys 0 (vaddr `MIN`), PostOrder → `MAX`. The root's traversal position is
   **invariant under promotion**: when a new root is promoted, it takes the old
   root's vaddr — `tree.root` does not change. The block spreads symmetrically
   (`i → 2i`), so for InOrder the root stays at phys `len/2`.

3. **Physical slot order == traversal (walk) order.** THE invariant the moved-ptr
   fixup relies on. The moved physical run is a contiguous in-order run, so
   `next()`/`prev()` enumerate it and `parent()` yields each moved node's
   parent. If this breaks, the fixup's `next()` ascends into/past the root,
   the ancestor stack empties, and `parent().unwrap()` panics (the symptom).
   Splits that create new internal nodes over existing subtrees currently break
   this — that is the open problem this plan solves.

4. **Root is pinned during `insert_child`.** `find_slot`/`slide_none` are
   pin-clamped (`pin` keeps slides off the root slot), so a slide never covers
   the root and the fixup's `next()`/`parent()` ascent is bounded below the
   root. This only holds if `self.root()` is the *actual* root vaddr — which
   invariant 2 (insert_root keeps `tree.root` put) guarantees.

5. **Proactive top-down split; non-root nodes are non-full when visited.** Every
   non-root node was split by its parent before descent, so only the root can be
   full when the walker sits at it. Overflow therefore never propagates more
   than one level per insert.

6. **Split-on-overflow for the root.** The root splits only when it *gains a
   child this insert*: (a) terminal root full + a new separator would be added,
   or (b) internal root full + the immediate child we'd descend into is full
   (its split would push a separator into the full root). A full root that gains
   no child this insert (overwrite, or descent into a non-full child) is left
   alone. (With DEGREE=2 a freshly-split root is full again, so the old
   unconditional `if root.is_full() split_root()` fired every insert and never
   converged — see bug #1.)

## Bugs fixed this session (already in the working tree)

1. **`UBTree::insert` split every insert (DEGREE=2 self-defeat).** Was
   `if root.is_full() { split_root() }` unconditionally at the top. With
   `DEGREE=2` a split root holds 2 children = full, so every insert re-split.
   Fixed to split-on-overflow (invariant 6): an outer `'split` loop re-creates
   the walker after a `split_root`; the inner loop is the persistent-walker
   descent. `split_root` is called only in the two overflow cases above, then
   `continue 'split` re-routes from the new root.

2. **`Walker::insert_root` `skip==1` off-by-one.** The branch did
   `set_root(p2v(anchor_p.wrapping_add(delta)))` — a double step. `anchor_p` is
   already `anchor_p0 + delta` (the anchor/old-root's new phys after sliding one
   slot toward `from`); the extra `+delta` tracked the old root to the wrong
   vaddr (and, in the first configuration encountered, to `P::MIN`, which we
   initially misread as a sentinel — see invariant 1). Now `p2v(anchor_p)`.

3. **`Walker::insert_root` was missing the `skip==0` pre-advance branch.**
   `insert_child`'s fixup has a `skip==0` branch (`next`/`prev` +
   `anchor_p += delta`) before the loop; `insert_root` lacked it and advanced
   at the top of the loop instead, mis-indexing the moved run when the anchor
   didn't move. Mirrored `insert_child`'s structure (skip==0 and skip==1
   branches both pre-advance to `anchor_p0 + 2*delta`; loop advances at the
   bottom).

4. **`Walker::insert_root` moved `tree.root` and reversed the new root off the
   root slot.** It `set_root`-tracked the old root (so `tree.root` left the root
   vaddr) and unconditionally `swap(open_v, root_v)`-ed, which in the
   None-left case swapped the new root (already at MIDPOINT) back to the old
   root's Before slot. Rewrote per the agreed recipe:
   - new root takes the old root's vaddr — `tree.root` **unchanged** (invariant 2);
   - old root tracked in a **local** `old_root_new_v` (NOT via `set_root`), returned for the caller to wire as child 0;
   - `if open_v == root()` → insert the new root directly (the slide freed the root slot); else insert at the open slot then `swap(new_v, root())`, and `old_root_new_v = open_v`.

   Result: `split_root` #1 (terminal root → height 1) now produces a correct
   in-order layout: `slots: [0:[L0:0], 1:X, 2:[0,3], 3:[L10:1,L50:1]]`, root at
   MIDPOINT (phys 2 = len/2, vaddr unchanged), child 0 before, child 1 after.

## The open problem (what this plan implements)

When a split creates a **new internal node that adopts already-placed children**,
`insert_child` today places the new node at a free slot found relative to its
*parent* (`insert_position` → `After(parent_v)`), not relative to its *own
subtree*. The new parent and its adopted child end up separated → physical !=
in-order (invariant 3 breaks) → the next `split_child`'s `next()`/`parent()`
fixup walks into the root and panics.

Proof layout after `split_root` #2 (insert 30, genuine overflow), `shift=29`:
```
slots: [0:[L0:0], 1:[6], 2:[0], 3:X, 4:[2,1], 5:X, 6:[L10:1,L50:1], 7:X]
root phys 4 (MIDPOINT, len/2) keys=[10]
child0 = phys 2 ([0], h1) -> child [L0:0] at phys 0      in-order: 0, 2  (contiguous, ok)
child1 = phys 1 ([6], h1) -> child [L10:1,L50:1] at phys 6  in-order: 6, 1  (PARENT phys 1 BEFORE its child phys 6 — broken)
full in-order = 0,2,4,6,1   but physical = 0,1,2,4,6
```
The new internal node (child1, the split sibling) is at phys 1 but its adopted
child is at phys 6 — placed by parent-relative anchor, not subtree-relative.

## Design (agreed)

**Subtree-aware `insert_position`.** "before child[i]" means before the
**leftmost descendant** of child[i]; "after child[i]" means after the
**rightmost descendant** of child[i]. The anchor is the subtree boundary, not
the immediate child. The old fixed "parent after `DEGREE/2`" rule is dead —
placement is determined by actual subtree extremities.

**A node that gets a new child moves to the position after
`child[nchildren/2]`'s rightmost descendant.** (General rule.) So inserting a
child can move the parent *over other nodes*.

**The split operation** (child `i` splits into `i` and `i+1`; child `i` keeps
its left half, its right half becomes `i+1`'s subtree):
- free a slot after child `i`'s *new* (left-half) rightmost descendant;
- move the **parent** there (it hops from after child `i`'s old rightmost
  descendant to after its new one);
- the **new child `i+1`** takes the parent's old slot (now between child `i`'s
  left half and the relocated right-half subtree);
- the right-half subtree ends up to the right of the new child — in-order intact.

Concretely, as the user framed it: "free space for insert after child-4's
rightmost descendant, move parent there, new child takes parent's old spot."

**New primitives required.**
1. **`leftmost` / `rightmost` walks** on the probe/walker (a `TreeRoute`/`TreeNav`
   addition): descend always taking child 0 / child `k-1` until terminal. Pure
   reads — testable in isolation. `insert_position` uses them to resolve anchors;
   splits use them to find hop targets.
2. **Block-level node split** — formally relocate a node's right-half subtree and
   open a slot (with the parent hop + new-child-takes-old-slot above), instead of
   the current "place a new node at the nearest free None." This is where the
   moved-ptr fixup actually has a contiguous in-order run to walk.

## Implementation order

1. **`leftmost`/`rightmost` walk primitives** on `TreeRoute`/`TreeNav` (+ tests).
   Unblocks subtree-aware `insert_position`; pure reads.
2. **Subtree-aware `insert_position`** — resolve before/after to subtree
   extremities via the walks above. The receiver is the node being inserted
   against (parent), so the walks read the parent's child subtrees.
3. **Block-level node split** — the subtree-relocate + parent-hop + open-slot
   primitive. Decide the signature: likely a `Walker` method that, given the
   child being split, performs the relocate + hop + returns the new child's
   vaddr and the moved-run fixup.
4. **Rewire `insert_child`/`split_child`/`split_root`** on top of (2)+(3):
   - `insert_child`: anchor at subtree boundary (2); on placing a node that has
     its own subtree, place adjacent to that subtree.
   - `split_child`/`split_root`: use the block-level split (3) instead of
     "copy right half + insert_child + manual wire."
5. Re-enable `ubtree_insert_many` (currently `#[ignore]`) once the moved-ptr
   fixup survives a multi-level split.

## Test progress bar

- `ubtree_single_node`: currently fails at `split_root` #2's following
  `split_child` fixup (`parent().unwrap()` on `None`) — the open problem.
  After this plan, should pass fully (insert/get/remove/range for 5 keys).
- `ubtree_root_split_only`, `ubtree_get_mut`, `probe_maps_key_to_lptr`,
  `debug_layout_demo`: check still pass.
- `ubtree_insert_many` (`#[ignore]`): the real multi-level stress test; un-ignore
  after the plan lands.

## Notes for the next agent

- The working tree already contains fixes 1–4 above; do not revert them.
- `eprintln!` debug lines prefixed `[fixup]`/`[next]` in `tree_traits.rs` and
  `lib.rs` are pre-existing debug instrumentation — leave or remove as you see
  fit, but they're not the bug.
- `DEGREE = 2` is intentional ("small for easy tracing"); the split-on-overflow
  policy (invariant 6) is what makes it workable, not bumping the degree.
- When tracing layout, remember vaddr↔phys depends on the current translator
  (`shift`/`offset`/`rotation`); spreads halve `shift` and remap `i → 2i`, and
  vaddrs are stable across spreads (re-translate pos/pin after a spread — see
  `block.rs::find_slot`).