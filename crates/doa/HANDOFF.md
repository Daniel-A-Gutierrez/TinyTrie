# doa handoff — current state, known flaws, agreed directions (zero-context)

For an agent with no conversation context. **Read `CLAUDE.md` and
`subtle_bugs.md` in this directory before touching code** — they carry the
worldview and the invariants. This document uses their vocabulary freely.

## 0. Vocabulary (60 seconds)

doa stores whole trees in dense arrays. Two facts carry everything:

- **addr vs pos**: a node's *addr* (u16/u32) is a stable name assigned at
  birth, stable across capacity growth. Its *pos* is the slot it occupies
  right now. A translator maps between them. Mutations move pos, never addr;
  growth (spread) remaps pos en masse but the translator absorbs it so addrs
  hold still. Position order is the only real order (pos 0 = smallest).
- **The fixup protocol**: every slot-moving mutation returns a closed-form
  correction (`NoneSlide` = "run [from,to] shifted by delta", `GrewFixup` =
  "positions remapped", `DoubleSlide` = two disjoint slides, `SwapFixup`).
  Anything holding addrs or positions implements `Fixable`
  (`grew_fix`/`swap_fix`/`slide_fix`/`two_slide`) and applies what it
  receives. The crate never hunts down pointers; it reports.

Walker stack (`src/walker.rs`):

- **Consumer-implemented mask**: `NodeCursor` (reads: `block`, `position`,
  `is_root`, `is_leaf`, `child_count`, `child`, `children`, `lookup ->
  Option<ChildPos>`; consumer-impl'd `descend`; defaults `current`/`search`),
  `NodeWalker` (`ascend`/`parent`), `NodeWalkerMut` (`Fixable` supertrait +
  `block_mut`/`set_child`/`clear_child`/`set_parent` + the internal state
  machinery `Snapshot`/`save`/`load`/`set_position`).
- **Crate-implemented**: `TreeWalker<O, NW>` wraps the consumer's `NW`;
  `TreeWalk` (per-ordering traversal); `PreOrderWalk`/`InOrderWalk` — the
  **open surface**: `open_here`/`open_child`/`open_2_child` (preorder adds
  `open_parent`/`open_parent_child`; in-order adds
  `rotate_left`/`rotate_right`). Opens **consume the walker** and return
  `(OpenSlot(s), OpenFixups)`; consumers holding addrs across an open apply
  the returned fixups (held addrs need only the slides/gather — addrs are
  grow-stable; held positions need grew too). `open_n_here`/`open_n_child`
  open n contiguous slots from ONE gather (scattered Nones crossed by a
  single move; `OpenFixups.gather` carries the plan).
  Rotations follow the **stand-on-the-riser contract**: the walker starts on
  the riser and ends where it started (position and ancestry); no
  `set_position`.
- **Internal** (`pub(crate) trait TreeWalkHelper`): `fixup` (the run-parent
  walk, pre-slide, with canaries), `apply_slide` (fixup -> `slide_none` ->
  `slide_fix` -> `reparent_run`), `walk_to_anchor`/`back_from_anchor`,
  `open_at`/`open_2_at`.

The invariant under everything: **walk order == slot order**. The per-visit
+ endpoint asserts in `fixup`'s run walk are the canary (subtle_bugs §6) —
do not weaken them.

## 1. What landed (commit a5544dd, 2026-09-19)

The walker-surface refactor: the crate no longer drives tree mutations — it
opens slots at ordering-correct anchors; consumers do all wiring (inserts,
drains, reparenting, root promotion) via the block directly. Splits are
consumer-driven and preemptive; working references: `examples/btree.rs`
(B+ consumer, DEGREE 6) and `src/tests/walker.rs` (B+ DEGREE 4 + a DEGREE=2
in-order binary consumer; 8 tests, miri-clean). Also landed: a find-ladder
fix — a full-len scan rung in Uniform/Anchored `find_slot` before the forced
spread (append-heavy growth densifies the store edge past the fixed 16-slot
budget while mid-span holes remain; without it, forced spreads run to
shift-exhaustion). A fresh review agent passed over the diff; its one real
finding (a held position crossing an open uncorrected in the childless-root
split flow) is fixed.

## 1a. What landed second session (2026-09-19, later same day)

Three directions from §4, all miri-clean (11 tests):

1. **Rotations → stand-on-the-riser (§4a, implemented)**. Choreography per
   §4a below, deviations found by the fresh review + one kept write:
   - the pseudocode omits the riser's own stored-parent-field write — kept as
     `set_parent(g_a)` while standing on the riser (non-root case), matching
     the old code's third write (reviewer verified sound).
   - the pseudocode's "end: on L, where it started" is POSITION-ONLY; the
     end STATE must be tree-truth: the riser ROSE a level, so its true path is
     one shallower than the walker's descent history. The shipped ending pops
     the transient entry (`ascend` + `set_position` — a state-only restore
     over the fully-consistent post-rotation tree; NOT the old
     mid-choreography reach). Root case ends with an empty stack (is_root
     true); mid-tree ends with the true parent entry. Without this the
     rotation's own walker reports a stale parent (root: is_root() lies).
   Tests reworked: descend to the riser first, rotate, assert end position ==
   start + end-state truth (is_root / the true parent entry).
2. **§4b, partial**: `set_parent -> Option<B::A>` (the overwritten old parent
   addr, `None` for parent-free shapes) — the walker does what it likes with
   it; crate call sites discard. `Ancestry` inline-backed (8 entries + heap
   spill, Index/IndexMut, `mem::replace` pop): the per-slide save/load clone
   is a fixed-size copy below depth 8. The CLONE STAYS — for stack walkers
   it is irreducible (§5: the walk pops the anchor's own path entries;
   re-deriving walks through rewritten entries), so the reachable win was the
   allocation, which is gone. The ascend-by-old walk-back USAGE remains
   future work, and is unvalidatable until a parent-field consumer exists.
3. **Gather machinery v1** (the old §"find_n_slots/open_n" debt):
   `GatherSlide` — N-None gather as a closed-form crossing-count fixup
   (`delta(pos) = ±#{holes beyond pos}`; NOT sequential NoneSlides —
   overlapping runs' coordinates go stale mid-application, same reason
   DoubleSlide requires disjointness). Store tier: `find_n_slots`
   (rel-side nearest-n, pin-clamped — v1: holes only on the open's side) +
   `gather_none` — one directional compaction pass (After descends moving
   members up, Before ascends moving them down; each slot's own iteration
   precedes any write into it, and member finals are distinct), every
   member Some crosses the run exactly once. `BlockOps` ladder: budgeted →
   full-len → spread+rescan → exhaustion. Walker: `fixup_gather` (one
   single-direction run walk over the member interval — (anchor, q_max) /
   (q_min, ANCHOR): the Before side extends to the anchor-adjacent slot —
   per-visit + endpoint canaries, snapshot/restore), `apply_gather` (fixup →
   gather_none →
   state gather_fix → `reparent_range` — reparent_run's body at explicit
   Some-dense post bounds; the Before bound saturates + skips the
   empty-member edge), `open_n_at` + `open_n_here`/`open_n_child` on
   both orderings. `OpenFixups.gather` (struct no longer Copy; held
   positions apply `grew` FIRST, then slides/gather);
   `Fixable::gather_fix` (by-ref). Known v1 scope holes (deliberate):
   mixed-side gathers, Pluripotent edge-grow for n>1, Uniform's proactive
   3/4-spread rung in the ladder, the in-order `open_n_*` surface
   (untestable against the binary consumer — any 2-node burst needs slots
   on both sides of the parent gap or yields leftless-right chains).

## 2. Where the surface debate landed

The mask-flattening was the point of the refactor (`State` assoc + accessors,
`parts`/`parts_mut`, `reposition`, `record_descent`, `has_space`,
`current_mut`, walker-level `insert_child`/`remove_child`, `SplittableNode`,
`Node::Payload` — all deleted; `lookup -> Option<ChildPos>`; `children() ->
(ChildPos, A)` pairs). A further subtraction was then proposed — deleting the
remaining state machinery (`Snapshot`/`save`/`load`/`set_position`) — and
**rejected after review**:

- **`save`/`load` stays.** `fixup`'s run walk rewrites each moved member's
  parent→child entry (and its stored parent field) to POST-slide addrs over
  the PRE-slide layout; walking back would re-descend through those
  rewritten entries (subtle_bugs §5). The snapshot/restore is a correctness
  instrument, not a convenience. The per-slide state clone is an accepted
  cost — an optimization target, not a subtraction (§4b).
- **`set_position` stays for now** — two uses: `reparent_run`'s member jumps
  and the rotations' end-of-op pop (post-§4a: a state-only restore over the
  consistent post-rotation tree, uniform across root/mid-tree — NOT the old
  mid-choreography reach, which is what §4a killed).
- **`clear_child` and `set_parent` stay** (explicitly kept: masked write
  primitives the rotations need; `set_parent` now returns the overwritten
  old parent addr).

## 3. Known bugs / flaws (current code, review findings applied)

*Latent, correctness-flavored (none firing today):*

1. `reparent_run` jumps `set_position` to each moved member — sound only
   because the post-slide layout is consistent; `STORES_PARENTS`-gated, so
   zero consumers exercise it. (The rotation root-case use is GONE — §4a
   landed.)
2. ~~rotations' root case uses `set_position`~~ — RESOLVED by §4a.
3. A unary internal riser panics in `rotate_right` (slot 1 undefined by the
   `child` contract); guarded by `debug_assert` only.
4. Packed-ChildPos nav: all traversal assumes rank == slot. Sparse-addressed
   nodes — exactly what a leaf-riser rotation produces (right-only binary
   nodes) — cannot be `TreeWalk`'d; tests reference-walk them. General fix =
   children()-based sibling walks, unwired.
5. `STORES_PARENTS` walkers are an unvalidated hazard class: a walker whose
   `ascend` reads node parent fields cannot ascend mid-fixup (the fields are
   rewritten to post-slide addrs). The crate's own state is snapshot-guarded,
   but no such consumer exists to test the whole path. (The new
   `set_parent -> Some(old)` return is equally unexercised.)
6. `bst_insert` (test harness) can build right-only nodes on a right-first
   insert order — the in-order convention cannot represent that layout;
   asserted loudly, but it is a harness trap, not a crate guard.

*Design-debt, deferred by agreement:*

7. fixup's per-slide state clone — STAYS (irreducible for stack walkers, §5);
   the heap alloc is gone (inline `Ancestry`). The §4b walk-back usage is the
   remaining lever and needs a parent-field consumer first.
8. ~~`find_n_slots`/`open_n`~~ — LANDED (gather v1; mixed-side gathers and
   the store-test matrices remain future work).
9. `split_block` body `todo!()` — arena tier.
10. Store tests unwired — the port never happened; find/slide/gather
    matrices dark.
11. `children()`'s sortedness contract (ChildPos sorts like keys) is
    unchecked — nothing validates it.
12. The full-len scan rung in the find ladders (`blocks.rs`) — correct (it
    killed the append-heavy spread runaway) but an O(len) scan on every
    budget miss before spreading; unbenchmarked. (`find_n_slots`'s default
    ladder has the same rung.)

Verified sound under adversarial review (do not re-flag): fixup's walk
geometry + canaries, the open engines' grew/re-walk ordering, the rotations'
rewire order, the ladder rung's exhaustion semantics, the no-op `b` slide in
single-open `OpenFixups`.

## 4. Agreed directions

### a. Rotations: the stand-on-the-riser contract — IMPLEMENTED (see §1a)

Kept below as the record of the contract now enforced by precondition
asserts and tests: the walker starts ON the riser, all writes via
up-navigation, ends where it started (the choreography below is what shipped,
plus the riser's own parent-field write the pseudocode omitted).

The walker starts ON the riser (the child that rises: child 0 for
`rotate_right`, child 1 for `rotate_left`). All writes go via up-navigation
(`set_child` with `up=1/2` over the ancestry stack); the one `set_parent`
happens during a brief ascend-to-P-and-back while P's slot still names the
riser; the block root follows via `data_mut().set_root` in the same visit
when P was the root. The walker ends where it started — root case and
mid-tree case uniform, no `set_position`, and the natural AVL/treap
ergonomics (rebalancing continues at the subtree root).

`rotate_right` choreography, exactly (`rotate_left` mirrors with 0↔1):

```
precondition asserts:  parent() = Some((_, ChildPos(0)))
                      unary-internal riser guard: is_leaf() || child_count() == 2
reads (no movement):   l_pos = position(); l_a = p2a(l_pos)
                       (p_pos, _) from parent(); p_a = p2a(p_pos)
                       lr_a = if is_leaf() { None } else { child(ChildPos(1)) }
ascend                — walker names P:
  set_parent(l_a)                     — P's field names L
  match parent(): Some((_, gp_idx)) => set_child(1, gp_idx, l_a)  — G's entry
                  None               => block data_mut().set_root(l_pos) — P was root
  descend(ChildPos(0)) — walker names L (P's slot 0 STILL names it)
if lr_a.is_some():
  descend(ChildPos(1)) — LR (L's slot 1 still names LR)
  set_parent(p_a); ascend() — walker names L
match lr_a: Some(lr) => set_child(1, ChildPos(0), lr)   — P's slot 0 takes LR
            None     => clear_child(1, ChildPos(0))     — riser was a leaf
set_child(0, ChildPos(1), p_a)                          — L's slot 1 = P
end: on L, where it started (in-order rotation: nothing moves physically)
```

Test rework: position the walker on the riser first (`descend` to child
0/1), rotate, assert end position == start position plus the existing
structure/sequence/root asserts.

### b. fixup walk-back optimization — PARTIAL (the return landed; the usage didn't)

`set_parent -> Option<B::A>` is IN: the overwritten old parent addr (None
for parent-free shapes). The crate discards it; what a walker does with it
is walker territory (its own `save`/`load` may journal pops, or a
parent-field walker may ascend-by-old). The ascend-by-old WALK-BACK inside
fixup remains future work — and unvalidatable until a parent-field consumer
exists (the hazard class of §3.5). What also landed toward this: inline
`Ancestry` — the per-slide snapshot/restore no longer heap-allocates below
depth 8; the clone itself is irreducible for stack walkers (the walk pops
the anchor's own path entries; §5).

## 5. Execution rules + verification

- Both slides are computed before either moves (subtle_bugs §2); no walk
  runs in a transiently invalid window (§1) — any new walk runs pre-slide
  over the valid tree or post-slide over the consistent layout.
- The run-walk canaries stay (§6).
- Comment every walker-move call (`//walker: -> names X`) — choreography
  reads as choreography.
- Targeted tests only: `cargo test -p doa --lib walker::tests` (full-crate
  runs have OOM'd the IDE); `cargo run --example btree`;
  `cargo +nightly miri test -p doa --lib walker::tests` (slow) before
  committing changes to the choreography.