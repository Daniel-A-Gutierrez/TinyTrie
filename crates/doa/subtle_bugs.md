# doa — subtle bugs, and the rules they left behind

Nuanced correctness issues hit while building the crate, each with the trap, a
diagram, why it was easy to miss, and the fix. The point is not history — it's
that each of these generalizes to a rule, and the rule now lives in the code.

Cross-referenced from `CLAUDE.md`; the operational summary of the rules is there,
the reasoning is here.

---

## 1. The postorder root split: walking a diverged tree

**The trap.** Postorder walk order is *children, then node*, and physical slot
order equals walk order. When an internal root R splits, R must relocate to the
mid boundary (its kept half's edge) — but the old flow relocated R *before*
draining, then opened Y's slot, whose slide runs a run-parent-fixup **walk**:

```
BEFORE — R owns [A,B,C,D]; postorder puts R after everything:

   slots:   [ A ][ B ][ C ][ D ][ R ]
   walk:      A    B    C    D    R          ✓ walk order == slot order

old flow, step 1: swap R to the mid boundary (R keeps [A,B]):

   slots:   [ A ][ B ][ R ][ C ][ D ][ .. ]
                       ^ R's position is only valid for its POST-split
                         children [A,B] — but R still OWNS all four!

old flow, step 2: open Y's slot → slide → fixup WALK. the walk traverses by
child pointers; walk order still says R follows D, physically it doesn't:

   walk order ≠ slot order ⇒ the walk lands on wrong nodes and rewrites
   entries with wrong vaddrs — corruption, silently.
```

**Why it was easy to miss:** the window is transient — the tree is consistent
again after the drain — and preorder/in-order splits have no such window (a
preorder node's position is valid with any tail of children; in-order's R never
moves). Only postorder's node-last convention makes "relocated before drained"
invalid in *both* orderings of the two steps. No consumer ever ran it.

**The fix:** open both slots *while the tree is fully consistent*, then do all
mutation (drain, swap) with no walk in the window — see §2. The postorder root
split now uses `find_2_slots`/`open_two` for exactly this.

---

## 2. No outstanding reservations across a walk

**The trap.** An open-but-unwritten slot can be *stolen* by the next
`find_slot`; a written one can't. With one None in play between two anchors,
any number of sequential `find_slot` calls juggle — and destroy — the first
reservation:

```
one None, two reservations wanted:

   slots:   [ x ][ y ][ · ][ z ]

   find_slot(x, after) slides the None adjacent to x:
   slots:   [ x ][ · ][ y' ][ z ]           reservation OPEN at x+1

   find_slot(y', after) finds the SAME None:
   slots:   [ x ][ y ][ · ][ z ]            the first reservation is GONE

a WRITTEN slot can't be stolen (find_slot only moves Nones):
   slots:   [ x ][ N ][ y' ][ z ]           N = written node — safe ✓
```

**Why it was easy to miss:** sequential single-slot reasoning ("find_slot
always opens me a slot") hides the interference; it only appears when two
reservations must coexist, which the postorder root split was the first to
need.

**The fix and the rule:** two reservations are opened atomically
(`find_2_slots` — sphere scan + interference test) and **every flow's
discipline is: write each slot before the next opens, or reserve all slots up
front**. Which one a flow can use is decidable from its transient window:
if some ordering of the mutations keeps the tree walk-safe, write-early
suffices; if *no* ordering does (postorder's internal root split), all slots
must be reserved before any mutation.

**Addendum (found by the postorder leaf root split): a WRITTEN slot can't be
stolen, but it can MOVE.** An intermediate `find_slot` between opening a slot
and using it may grow (spread), remapping every live phys — the walker state
gets the `found.grew` fixup, but so must every `OpenSlot` and every captured
node phys the flow still holds (the postorder leaf split's `y_open`/`r_phys`,
the internal split's `r_phys` after `open_two`). Miss one and the flow adopts
or drains at the vacated slot — a `None`-read panic at best, corruption at
worst. The rule: a `find_slot` inside a flow is a phys-remapping event for
everything the flow holds, not just for the walker.

---

## 3. Reparenting during the run walk: post-slide entries, pre-slide layout

**The trap.** The NoneSlide fixup walks the moved run *before* the slide and
rewrites each moved node's parent-entry to its **post-slide** vaddr as it goes.
Parent-storing shapes also need moved nodes' *children's* parent fields fixed —
and doing that inside the walk (descend to each child, `set_parent`) descends
through the node's entries, which by then are a **mix**:

```
in-order run walk, delta = +1 (items shift right). X is visited mid-run:

   slots:   [ · ][ C ][ X ][ D ]
             ^None

   X's entry for C:   C was visited EARLIER in the walk
                      → already rewritten to C's POST-slide vaddr
   X's entry for D:   D not yet visited → still D's PRE-slide vaddr

   descend via entry C → v2p(post vaddr) = C_old+1 → the WRONG slot,
   pre-slide. descend via entry D → correct. mixed!
```

**Why it was easy to miss:** the pre/post mixture depends on walk order — a
parent-storing consumer in preorder would have gotten away with it (parents
precede children, so no entry is rewritten before the parent's visit). It
breaks for in-order and postorder. (Caught in design, before any
parent-storing consumer existed.)

**The fix:** reparent **post-slide, position-based over the shifted run**
(`reparent_run` in `apply_slide`). Post-slide, *every* entry is consistent —
in-run children's entries were rewritten to where they now are, out-of-run
children never moved. No collection, no fixup-of-pointers needed: the slide
itself does what a collected-Vec fixup would have.

---

## 4. The root hop corrupting `BlockData::root`

**The trap.** The in-order hop relocates a node whose boundary identity
shifted (§8). Swaps emit no self-fixup — the mover applies `SwapFixup` by hand.
The hop fixed the walker state and the grandparent's entry, but when the
hoppee was *the block's root* (a root parent in in-order), nothing updated
the block data's root phys:

```
in-order hop of a root parent (left split shifted its boundary):

   before:  data.root ──┐
   slots:      [ A ][ R ][ B ]        R = tree root AND block root
                          ↑ hop target: before child[b]

   after:   slots: [ A ][ R' ][ B ]   data.root STILL points at R's
            data.root ──┘ (dangling)  old phys — every fresh walker
                                      (constructed from data().root)
                                      starts on garbage.
```

**Why it was easy to miss:** the hop's guard was `parent().is_none()` ⇒ "skip
the grandparent repoint" — which read as *nothing to do* at the root, when
there was in fact a different owner to fix (the block data, not a
grandparent). No in-order consumer existed to hit it.

**The fix:** `hop_current` checks `parent().is_none() && data().root() ==
from` — the hoppee being the block root — and calls `set_root`. `swap_current`
deliberately does *not* own this (it has no `HasRoot`); the tree-level caller
does.

---

## 5. Walking back after the run walk: rewritten pointers, unslid layout

**The trap.** The fixup's run walk ends at the run's far edge, but the caller
needs the walker back at the anchor. Walking back ascends and re-descends
through child entries — and the walk has already rewritten some of them to
**post-slide** vaddrs, over a still-**pre-slide** layout:

```
slide: None moves to the run's near edge (delta +1). the walk visits C, X, D;
after visiting C, C's parent entry holds C's POST vaddr.

   slots:   [ · ][ C ][ X ][ D ]
   walk back: ascend from D → descend ... through the rewritten entry:
   v2p(post vaddr) names the wrong slot pre-slide.
```

**The fix:** the walker state is **snapshotted at the anchor and restored**
after the walk — zero walking. The walk itself stays forward-only, which is
what makes it sound: a forward-only walk can't re-enter a processed node's
subtree, so the entries it reads on the way are only ever unprocessed
(correct) ones.

---

## 6. The walk == slot-order canary

**The trap.** A reserved-but-never-wired `Some` (alloc without write/wire — a
consumer bug, or a bug in a flow) is invisible to the tree: no child entry
names it, so no walk ever visits it. Later, a slide shifts it like any other
Some, a fixup walk skips it, and eventually some `assume_init`-backed read
returns it as a node — garbage-as-`T`, i.e. UB, far from the cause.

```
a ghost Some (G) inside a run being walked:

   slots:   [ A ][ G ][ B ][ C ][ · ]     G not wired into the tree
   the walk visits only WIRED slots — next() skips G:
   steps = hi - lo covers 4 slots, but the walk can't land on G, so a visit
   lands outside the run (the walk runs long past the far edge).
```

**The fix:** two always-on asserts in the fixup's run walk. Per visit: every
one of the `steps` visits must land inside the closed run `[lo, hi]` — the run
is None-free by `find_slot`'s construction, and the walk is forward-only, so
`steps` in-range visits are exactly the members. At the end: the walk's
position must be the run's far edge exactly — against a consistent layout the
walk visits the run in slot order, so any ghost lands the endpoint short or
long. Both fire **at the moment of the inconsistency**. Cheap (integer
compares) and load-bearing precisely because the store is `MaybeUninit`-backed
(§7) and the failure mode is otherwise UB rather than a panic.

The endpoint check has ONE sanctioned skew: the in-order hop's slide (§11)
walks its run in LOGICAL order — the misplaced hoppee is visited first — so
when the hoppee is itself the far-edge member the walk ends one below far.
`fixup`/`apply_slide` take a `far_short` parameter the hop passes exactly
then; every other caller passes false and gets the strict invariant. (The
skew is only one: a mid-run hoppee is visited first but the walk still ends
on the far member.)

---

## 7. MaybeUninit slots: the drop leak, and why no transmute

**Representation.** Slots are `Option<MaybeUninit<T>>`: the discriminant is
the occupancy flag (store-internal, flipped only by `alloc`), the payload is
exempt from validity until its reservation's write completes (**alloc-write-read**
— enforced by the exclusive `&mut MaybeUninit<T>` handed out).

**The drop leak.** `MaybeUninit<T>` never drops `T`, so dropping a store drops
every node's *wrapper* but not its payloads — a regression from
`Vec<Option<T>>`, silent for Copy-ish nodes, a leak for heap-carrying ones.
Fixed with `Drop` impls on both stores: `assume_init_drop` over every `Some`.
Sound *because* of alloc-write-read; the one dangerous case is documented on
the impl: unwinding through a pending reservation (a `Some` not yet written)
is UB — the contract's single sharp edge.

**Why the obvious transmute doesn't work** (handing `&mut MaybeUninit<T>` out
of a `None` `Option<T>` slot):

1. **Layouts differ.** `MaybeUninit<T>` is niche-proof — every bit pattern is
   valid — so `Option<MaybeUninit<T>>` is always `tag + size_of::<T>()`, while
   `Option<T>` is often niche-packed to exactly `size_of::<T>()`. For
   niche-having `T` the sizes don't even match.
2. **Validity is not lazy.** The memory's true type is `Option<T>` (it lives in
   a `Vec<Option<T>>`). During the handoff the bytes would have to read as
   `Some` through the punned view while the payload is garbage-as-`T` — a live
   `Option<T>` holding an invalid value is UB whether or not anything reads
   it, and any drop glue on unwind would observe it. `MaybeUninit`'s exemption
   only works when the memory's *declared* type has no validity requirement —
   you can't get that by type-punning a view on top; the `Option<T>` view
   never goes away.

---

## 8. In-order position is fixed by DEGREE, and left inserts move the boundary

**The trap.** The original in-order convention was dynamic (`mid = cc>>1`),
which made a split *move the split node* — its boundary changes when it loses
half its children. Worse, the promoted root's placement was hardcoded
adjacent-right of R, which is only correct when the new root sits *between*
its two children:

```
NR's children after a root split: [R, Y]  ⇒  b_NR = min(2, DEGREE/2)

DEGREE = 3 → b = 1: NR sits BETWEEN its children:
   [ R ][ NR ][ Y ]                 adjacent-right of R ✓

DEGREE ≥ 4 → b = 2: NR sits AFTER ALL its children:
   [ R ][ Y ][ NR ]                 the REGION END — the adjacent-right-only
                                    code broke walk order here
```

**The fix — the convention, and why it's fixed:** a node sits between
`child[b-1]` and `child[b]`, `b = min(cc, DEGREE/2)`; `cc ≤ DEGREE/2` ⇒ the
node sits after all children. Fixed by DEGREE, not cc, precisely so that **a
full node's boundary is exactly its kept-left-half's edge** — splits never
move the split node, and the boundary's *identity* (who child[b-1]/[b] are)
only shifts when a **left** child is inserted or split (`slot < DEGREE/2`, a
pure const test). Consequences that all fall out:

- splits: X never moves; the parent hops iff `child_idx < DEGREE/2` (guarded
  by the block-root case, §4);
- inserts: the same hop rule (the hop is *not* split-specific);
- below `DEGREE/2` children the node sits after-all and absorbs inserts —
  no hop.

---

## 9. Two slots, two fixup walks: why they can't compose

**The nuance.** `find_2_slots` hands back both slides pre-mutation so both
run-parent walks could run before both slides — which would allow one
composed state fixup. It's deliberately *not* done that way:

```
walk A, walk B, slide A, slide B:  unsound for parent-storing shapes.
   a B-run member's parent may live in A's run → during walk B its
   stored parent field is written against pre-slide-A positions →
   slide A moves the parent with no remaining walk to fix the field.

walk A, slide A, walk B, slide B:  sound — walk B sees post-slide-A
   positions. disjointness keeps anchor B valid across slide A.
```

`TwoSlide` (the composed *fixup type*) still exists — order-independent
address rewriting is correct for any holder applying fixups wholesale — but
the *applying* side always walks-and-slides interleaved. The type serves the
returned API contract (external vaddr holders get one `fixup` call), not the
internal flow.

---

## 10. Preorder `prev` of a first child

**The trap.** Preorder: a node precedes its children, so `prev` of a first
child is the *parent itself*. The old loop ascended looking for a previous
*sibling* and walked past `idx == 0` off the block instead of returning the
ascended-to parent — contradicting the doc, and firing the fixup run-walk
canary (§6) on a later insert.

```
   [ P ][ A ][ A' ][ B ]
   prev(A'): A' is a last child → ascend to A, descend B... fine.
   prev(A):  A is a FIRST child → ascend to P → return P.  (the old loop
             kept ascending past the root instead)
```

Fix: ascend *first*, descend into the previous sibling's subtree only when
`idx > 0`.

---

## 11. The hop's fixup walk: the anchor must follow the None

**The trap.** The in-order hop relocates the one node whose logical gap
(`in_boundary` over its post-insert children) no longer matches its physical
slot — mid-hop, walk order and slot order *legitimately* diverge at that
node. The hop's slot-opening slide therefore needs a fixup walk whose anchor
depends on **where the None landed relative to the hoppee**:

```
gap before child[b]; hoppee H physically past it; None found to the right of the gap:

case A — None BEYOND H (H inside the run):
   slots: [ .. child[b-1] ][ members.. ][ H ][ .. ][ ·None ]
   anchor LEFT edge (after subtree_last(child[b-1])): next() of child[b-1] IS
   H via the ancestry stack — no entry read, position-true — then descents run
   through unprocessed entries only. anchoring at the right edge instead, the
   walk starts inside the run and can never reach H (H is logically BEFORE
   child[b]) — it walks off the block.

case B — None BETWEEN gap and H (H outside the run):
   slots: [ .. child[b-1] ][ members.. ][ ·None ][ H ]
   anchor RIGHT edge (subtree_first(child[b]) — exactly the slide's `to`):
   the walk starts on the run's first member and never crosses H. anchoring
   at the left edge instead, next() of child[b-1] is H — a NON-member —
   consuming a visit and rewriting its entries with a phantom delta.

identity or None left of the anchor: the run is at/below the left anchor and
the walker is in it — left edge both times.
```

**The rule:** `hop_current` probes with the left-edge anchor, then picks the
walk side by comparing `ns.from` against the hoppee's phys. The general
principle is §1's — no walk may run over a node whose logical position
disagrees with its slot — except the hoppee itself, whose visit must be
arranged to be entry-free (via the stack) and first, or avoided entirely.
Case A's walk ends on the far edge as usual UNLESS the hoppee IS the far-edge
member (it was visited first, so the last logical member is one below) — the
one `far_short` skew of §6; `hop_current` passes it exactly then
(`ns.from == hoppee + 1 && steps > 1`).

---

## 12. In-order `prev`/`subtree_last`: an after-all node IS the region's last

**The trap.** The in-order impls reused postorder's `rightmost_leaf` for
`prev` and `subtree_last`. But in-order, a node at `b == cc` sits *after all*
its children — it is its region's LAST node, and a bare rightmost-leaf
descent walks right past it:

```
node X, cc = 2 ≤ DEGREE/2 ⇒ b == 2 ⇒ after-all:
   walk order:  [ child0's region ][ child1's region ][ X ]
   subtree_last = X — but rightmost_leaf returns child1's rightmost leaf,
   two slots short. prev() of the node after child1's region lands past X —
   X is never visited, and a fixup run walk over the region SKIPS it.
```

**Why it was easy to miss:** preorder's subtree-last is a rightmost leaf and
postorder's is the node itself — in-order is the *conditional* (stop on
after-all, descend right otherwise), so the borrowed helper is wrong exactly
when a rightmost-path internal is underfull. The fixup canary (§6) is what
surfaced it: the skipped member moved a walk visit out of the run.

**The fix:** in-order `subtree_last` descends `child[cc-1]` only while
`b < cc`, stopping on the after-all node; `prev` uses it (not the bare leaf
descent) in both its descend and ascend-loop arms.

---

## Appendix — API-level traps, closed earlier

- **`'walker`-tied ref returns.** Returning `&'walker` from `&self` methods on
  a mut-holding cursor is unimplementable in safe Rust (a `&'walker mut B`
  field can't vend `'walker` shared refs through `&self`). All ref returns tie
  to the elided borrow instead; the consumer struct keeps its own borrow
  lifetime.
- **E0283 qualified-path helpers.** Trait *parameters* used only in some
  methods dangle at call sites; consumers had to write fully-qualified paths
  per constructor. Killed by making `TreeBlock` a param-less marker and moving
  construction to free fns (`walker`/`search`) over `From` impls.
- **`Default` on the node type.** The crate can't know whether a fresh root
  should be a leaf or an internal — `Default` on `Node` was a footgun. The one
  constructor the crate needs is the consumer's `SplittableNode::new_root`.