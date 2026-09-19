# doa — subtle bugs, and the rules they left behind

Nuanced correctness issues hit while building the crate, each with the trap, a
diagram, why it was easy to miss, and the fix. The point is not history — it's
that each of these generalizes to a rule, and the rule now lives in the code.

Cross-referenced from `CLAUDE.md`; the operational summary of the rules is there,
the reasoning is here.

---

## 1. The postorder root split: walking a diverged tree

*(postorder walker impls are unimplemented since the open-surface refactor;
the rule survives it — it is why `open_2_at` computes both slides before
either moves, and why consumer-driven flows open everything before draining.)*

**The trap.** Postorder walk order is *children, then node*, and position order
equals walk order. When an internal root R splits, R must relocate to the
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
child pointers; walk order still says R follows D, positionally it doesn't:

   walk order ≠ slot order ⇒ the walk lands on wrong nodes and rewrites
   entries with wrong addrs — corruption, silently.
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
and using it may grow (spread), remapping every live pos — the walker state
gets the `found.grew` fixup, but so must every `OpenSlot` and every captured
node pos the flow still holds (the postorder leaf split's `y_open`/`r_pos`,
the internal split's `r_pos` after `open_two`). Miss one and the flow adopts
or drains at the vacated slot — a `None`-read panic at best, corruption at
worst. The rule: a `find_slot` inside a flow is a pos-remapping event for
everything the flow holds, not just for the walker.

---

## 3. Reparenting during the run walk: post-slide entries, pre-slide layout

**The trap.** The NoneSlide fixup walks the moved run *before* the slide and
rewrites each moved node's parent-entry to its **post-slide** addr as it goes.
Parent-storing shapes also need moved nodes' *children's* parent fields fixed —
and doing that inside the walk (descend to each child, `set_parent`) descends
through the node's entries, which by then are a **mix**:

```
in-order run walk, delta = +1 (items shift right). X is visited mid-run:

   slots:   [ · ][ C ][ X ][ D ]
             ^None

   X's entry for C:   C was visited EARLIER in the walk
                      → already rewritten to C's POST-slide addr
   X's entry for D:   D not yet visited → still D's PRE-slide addr

   descend via entry C → a2p(post addr) = C_old+1 → the WRONG slot,
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

## 4. Whoever moves the root owns the block-root fixup

*(recast after the open-surface refactor — the hop is gone; `rotate_left/right`
and consumer-driven root promotions are the live instances.)*

**The trap.** Swaps and slides emit fixups the mover must apply, and the
block's `Root` data is one of the holders — but nothing in the swap/slide
machinery applies it *for* the mover. Relocate the root and skip
`data_mut().set_root`, and every fresh walker (constructed from
`data().root()`) starts on garbage:

```
before:  data.root ──┐
slots:      [ A ][ R ][ B ]        R = tree root AND block root

after:   slots: [ A ][ R' ][ B ]   data.root STILL points at R's
          data.root ──┘ (dangling)  old position
```

**Why it's easy to miss:** the mover's own state gets fixed (it holds the
fixup), the parent entry gets repointed — the block data is the one holder the
mover doesn't naturally have in hand, and the corruption is silent until the
next walker construction.

**The rule:** every root relocation ends with `set_root` — the crate's
rotations do it at the root; consumer-driven promotions must (the btree
example's split_root flows); `swap_open`-based hops must too.

---

## 5. Walking back after the run walk: rewritten pointers, unslid layout

**The trap.** The fixup's run walk ends at the run's far edge, but the caller
needs the walker back at the anchor. Walking back ascends and re-descends
through child entries — and the walk has already rewritten some of them to
**post-slide** addrs, over a still-**pre-slide** layout:

```
slide: None moves to the run's near edge (delta +1). the walk visits C, X, D;
after visiting C, C's parent entry holds C's POST addr.

   slots:   [ · ][ C ][ X ][ D ]
   walk back: ascend from D → descend ... through the rewritten entry:
   a2p(post addr) names the wrong slot pre-slide.
```

**The fix:** the walker state is **snapshotted at the anchor and restored**
after the walk (`NodeWalkerMut::save`/`load` over the consumer's `Snapshot`) —
zero walking. The walk itself stays forward-only, which is
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
compares) and load-bearing: against a consistent layout they cannot fire, so
either firing names a real desync between the tree and the slots.

(The endpoint check once carried ONE sanctioned skew — the in-order hop's
slide walked its run in logical order; the hop is gone with the open-surface
refactor and the check is strict again.)

---

## 7. *(deleted)* MaybeUninit slots

The reservation model (`Option<MaybeUninit<T>>` write-places) is gone — slots
are plain `Option<T>`, values always initialized, so the drop-leak and
transmute-soundness section no longer applies. The canary (§6) remains: it
outlived the representation, catching ghost `Some`s that no walk can name.

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

- in-order is binary-only since the open-surface refactor; binaries never
  split, so the hop is gone from the crate. a consumer driving an in-order
  layout beyond binary owns the hop itself: a left insert/split
  (`slot < DEGREE/2`) shifts the boundary identity — open_2 + swap the parent
  across the crossed child, ending with the block-root fixup when it moved (§4).

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
returned API contract (external addr holders get one `fixup` call), not the
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

## 11. *(deleted)* The hop's fixup walk anchor

`hop_current` is gone (in-order is binary-only; hops are consumer-driven).
The rule it encoded is §1's — no walk may run over a node whose logical
position disagrees with its slot — and it applies unchanged to any
consumer-driven hop: pick the walk side by where the None landed relative to
the node being relocated.

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