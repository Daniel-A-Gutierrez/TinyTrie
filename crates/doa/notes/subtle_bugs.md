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

## 13. The gather's fixup shape: crossing counts, not slide sequences

**The trap.** An N-None gather looks like N `NoneSlide`s — n holes each
moving to its slot beside the anchor. Composing the per-hole `NoneSlide`
fixups sequentially is wrong: each slide's run is expressed in the
coordinates of the moment, and a holder applying slide k sees positions
already shifted by slides 1..k−1 — a Some inside two overlapping runs
double-shifts or skips. This is §9's non-composability with the
disjointness requirement removed: gather runs necessarily overlap (they
share the anchor-adjacent segment), which is why `DoubleSlide`'s trick
cannot scale up.

**The fix.** One closed-form remap: every member Some crosses the gathered
None-run exactly once, so its delta is a **crossing count** — After:
`delta(s) = #{holes > s}` for the Somes in `(anchor, q_max)`, Before the
mirror with the holes below. One `GatherSlide` carrying the hole list
computes any element's shift in O(n); the store's `gather_none` applies it
as ONE directional compaction pass. The pass's safety rule: members all
move one way, so iterate the member interval against the movement (After:
descending, Before: ascending) — every slot's own iteration precedes any
write into it, and member finals are distinct, so nothing double-moves and
nothing clobbers. The single-hole degenerate (n=1) is exactly `NoneSlide`,
which is how the formulas were validated.

---

## 14. "Ends where it started" is position-only; the state must be TREE-truth

**The trap.** The stand-on-the-riser rotation contract says the walker ends
where it started — and the natural reading is the full walker state. But the
walker's descent history describes the PRE-rotation tree: it reached the
riser by descending through the parent's slot 0/1, and that entry is exactly
what the rotation rewires. Ending with the descent entry intact makes the
rotation's own walker lie: at the root, `is_root()` is false for the new
root; mid-tree, `parent()` names the demoted node through a slot that no
longer points at the riser. A consumer loop (`while !is_root() { rotate }`,
AVL-style continuation at the subtree root) walks into the wrong node or
panics — far from the rotation, on state the rotation itself corrupted.

```
mid-tree, walker on L, stack [.., (G, gp_idx), (P, 0)]:
rotate: G's slot gp_idx := L; P's slot 0 := LR; L's slot 1 := P
   L's TRUE path is now [.., (G, gp_idx)] — one level SHALLOWER (L rose)
   the kept entry (P, 0) claims P's slot 0 still names L — false.
```

**The rule.** "Ends on the node it started on" ≠ "ends in the state it
started in": when the mutation changes the current node's DEPTH or PARENT,
the end state must be re-derived from the post-mutation tree, not inherited
from the walker's history. The rotation ends with `ascend` +
`set_position(riser)` — a state-only pop over the fully consistent
post-rotation tree (the one benign `set_position` use; the mid-choreography
reach through a half-rotated tree is what stays forbidden). Root case: the
stack empties (`is_root()` true). Tests assert the end state's truth
explicitly, not just its position.

---

## 15. *(open)* u8 addresses: `Half = u8` is a lie

**The known flaw.** `Addr::Half` exists for Pluripotent's cap math
(`MAX_CAP = 1 << Half::BIT_WIDTH`, `SHIFT = BIT_WIDTH/2`), and there is no u4,
so `impl_addr!(u8, u8)` names u8 its own half:

```
u8: MAX_CAP = 1 << 8 = 256 > the whole address space; SHIFT = 4
Pluripotent<u8>: MAX_CAP (256) overstates capacity 16× — only the
`addressable = 2^8 >> shift`-min guard in its find ladder keeps it honest.
```

**Why it stands:** u8 addressing is for the blockset tier (devlog: small
blocks, u8 addrs), whose translator logic (full-width shift-down + rotation
turns) doesn't use `Half` at all. Reshaping the knob (`Half` type → a
`HALF_BIT_WIDTH: u8` const, so u8 can name 4) is deferred until that
translator settles — then revisit both `Mode::MAX_CAP` and Pluripotent's
initial `SHIFT` under a real u8 consumer. Until then: u8 Pluripotent blocks
compile and run, but trust `addressable`, not `MAX_CAP`.

---

## 16. shift-then-mask: the shift eats the mask width

**The bug (2026-10-12, `Mask::next_clear`).** word-wise scans shift the
inverted word right by the start offset, then mask to the word's valid
tail. the first version masked `(1 << end) − 1` bits — but `end` (valid
bits in this word) counts from the WORD start, while the shift had already
dropped `off` of them:

```
bits = 200, from = 193: word 3, end = 8, off = 1
mask (1<<8)−1 keeps bits 0..8 of the shifted word = positions 193..=200
bit 200 is past `bits` (padded clear in storage, so inverted = clear)
→ next_clear(193) returned Some(200) for a fully-set tail
```

caught by the exhaustive naive-comparison test (every from ∈ 0..=bits) on
its first run — the per-position oracle pays for itself.

**The rule.** after `word >> off`, the tail mask is `(1 << (end − off)) − 1`
— mask width is measured from where the SHIFTED value starts, not from the
word start. `prev_clear` avoids the trap by masking the unshifted word
first (leading_zeros needs no shift); any scan that shifts then masks owns
this arithmetic.

---

## 17. *(open)* stale names alias the new owner — the name-transfer sharp edge

blockset's addr = the slot's canonical name, and a slide TRANSFERS names
(the mover adopts the vacated slot's name, giving up its own). The released
name isn't retired — the next insert takes it. So a holder that skipped a
`Deltas` application holds a STALE name, and that name still aliases a live
slot: `get` returns the wrong element and `remove` deletes the wrong one,
silently. No panic, no aliasing detection possible — `a2p` is injective and
the slot's name is by definition minted.

```
{10@0, 20@128}; insert 5 → 10 renamed 0→64, 5 named 0
a holder still holding 0 (skipped the delta) now names 5, not 10
```

**The rule.** unlike treeblock (birth-stable addrs, miss-applied fixups
leave you pointing at a hole), here the delta is not optional metadata —
every `Deltas` handed out by an insert must be applied before its addrs are
used. The contract is name swaps, and an unswapped name is a wrong name.
(An arena applying deltas at split/hand-off time is the intended holder.)

---

## 18. "Full" is a per-resource question: mask-full ≠ names-exhausted

**The bug (2026-10-12, blockset insert ladder).** once push_back growth
landed (sequential biases extend the allocation one slot at a time), the
block's cap stopped being a power of two tracking len. the old grow rung
still equated "no hole in the mask" with "turn now" — and the turn was
refused whenever `2·cap` would exceed the name space:

```
u8 names (space 256), stride torture: cap grows past 128 via push_backs
mask fills at cap 132 → old rung: grow → 2·132 = 264 > 256 → refused → Err
len is 132; 124 names unused — insert of key 152 failed on a half-empty block
```

**Why it was easy to miss:** with pow2 cap growth the two fullnesses
coincided (cap hits the ceiling exactly when names do); one-slot-at-a-time
extension broke the coincidence, and the failure surfaced as a plain
`Err(k)` — the same signal a genuine out-of-names returns.

**The rule.** every growth decision must name WHICH resource is exhausted:
the ladder now distinguishes mask-full (room to push a slot always exists —
mask full + len < ceiling ⟹ cap < ceiling) from name-ceiling (the early
`len ≥ max_capacity` check). `push_slot` is the single growth primitive;
`grow_rotate` is a layout change, not a capacity change.

---

## 19. The turn's +1 is the recycled rol bit: the wrap band

**The nuance (2026-10-13, rotation turns with a translator offset).** the
name-preserving turn maps pos via the translator composition
`q(p) = a2p_new(p2a_old(p))` — names preserved by construction, the old
rotation cancels (`ror r ∘ rol (r+1) = rol 1`). what's left is a ramp
`x = (p − offset) mod 2^w` rolled left one — and `rol 1` is NOT `2x mod 2^w`:
when x sits in the upper half of the name space, plain doubling discards the
top bit and `rol` recycles it to bit 0. that recycled bit is the `+1`:

```
q(p) = 2·p + [x ≥ 2^(w−1)]        x = (p − offset) mod 2^w — a rising ramp
ramp wraps at p = offset          → +1 switches OFF: junction gap 1, no hole
ramp crosses half at p = o−mid    → +1 switches ON:  junction gap 3, two holes
```

the flip-OFF junction is the pair holding addr MAX and addr MIN — adjacent
in the circular name space, so "no hole between them" is forced by name
preservation, not chosen. with the MIDPOINT refusal (cap ≤ 2^(w−1)) at most
one junction falls inside the array, so the band is one prefix
(`offset ≤ midpoint`: bits < offset land odd — `rotate_mask`'s ripple range
`[0, 2·min(offset,cap))`), one suffix (`offset > midpoint`: bits ≥
offset−midpoint land odd — ripple `[2·(o−mid), 2·cap)`), whole-range, or
empty.

**The implementation rule.** `rotate_mask` = plain interleave (every bit →
2p, a hole after every element) + ONE masked ripple of the band's target
range. the UP-ripple (forward ramp) shifts up a slot and needs no carry
chain: an even bit and its odd target are adjacent, and word boundaries
sit at odd global bits, so the shift never crosses a word — every word
moves independently. the DOWN-ripple (the reversed ramp, §20) shifts down
and DOES cross — a word's bit-0 element lands on the word below's bit 63 —
so the fall carries word by word (descending word loop; whenever a fall
arrives, bit 63 is provably inside the ripple range). the two-step
segment machinery the up-ripple replaced (saved boundary word, merge
masks, last-word preservation) was all carry-chain bookkeeping for a
carry that provably never happens — in the up-ripple only.

---

## 20. The reversal: names survive the death of their orientation

**The nuance (2026-10-13, the reversal tier).** flipping a block (pos 0 =
min ⇄ pos 0 = max) is two mirrors and a negated translator — no deltas, no
name churn, nothing for holders to apply. the array mirrors
(`data.reverse` + `mask.reverse`); the translator re-parameterizes so
`p2a_new(p) = p2a_old(cap−1−p)` — in the 2-knob form that is exactly
`offset → cap−1−offset` with the direction flag flipped: the formula
negates its position term (`(p − offset) ror r` → `(offset − p) ror r`).
names follow the mirrored slots BY CONSTRUCTION, so a pending `Deltas`
event (the gather/slide from the same insert) stays valid across the flip
— it speaks names, and names did not change.

the reversal turn (the negated ramp): `q(p) = 2·p − [(offset−p) mod 2^w ≥
2^(w−1)]` — the upper-half names still land odd, but one slot DOWN. under
reversal the name of p is `offset − p`, so the upper half of the name
space is the pos-SUFFIX `(offset, cap)` (offset < cap ≤ midpoint in every
reachable state — offset is name 0's slot). the map is monotone, and
q ≥ p holds (band members have p ≥ 1), so grow_rotate's descending pass
stays safe under either direction. the down-ripple's word-crossing fall
is §19's implementation-rule addition.

**The rules.**
- the flip is free; the trigger is not: `pos_sum·8 < len²`, len >
  4·FREE_STRIDE (pos_sum doubles at turns, reflects around len at flips).
  at len/4 a uniform stream's small early window flips spuriously (benched:
  4 flips on pure random, each echo at a larger scale), and a real front
  stream's mean sits ≈ 0, so the tighter line costs nothing real.
- the all-time doubled pos_sum is an intentional estimator: each doubling
  era votes (roughly) once, so the block judges by early behavior and
  gets stickier as it grows — the devlog's "biases develop aggressively
  early, harder to change later," by construction.
- `offset ≥ cap` states (only offset turns can make them) break the
  reversal turn's monotonicity/bounds — unreachable today; the
  offset-turn tier must respect `offset < cap` or generalize the band.
- turns work under ANY offset and either direction now (the `offset ≠ 0`
  refusal is gone); `rotate_mask`'s reversed branch asserts
  `offset < midpoint`, which reachable states guarantee via
  `offset < cap ≤ midpoint`.

---

## 21. Cached-word iterators: exhaustion must zero the cache

**The nuance (2026-10-14, the SetBits engines — caught by session review,
verified by probe).** `SetBits`/`SetBitsRev` cache the current mask word
across `next()` calls. both exhaustion sites matter and behave
differently: the YIELD-path refresh (`unwrap_or(0)`) clears correctly,
but the HOLE-skip path's `?` operator returns `None` BEFORE the stale
word is cleared — a re-poll (Iterator's contract allows arbitrarily
many `next()` calls after None) then tests the STALE word at the
boundary position and yields a position ≥ bits: through
`BlockSet::iter` that is an index-OOB panic in `key()`. the first fix
attempt failed the same way — `None => return None` inside a match that
assigns the word only on `Some` still leaves the stale cache; the
exhaustion arm must explicitly zero `self.word` before returning.

**The rules.**
- any iterator whose state caches a slice element must zero/drop the
  cache at EVERY exhaustion site, not just the convenient ones. `?` on
  the fetch is the trap: it early-returns with state intact.
- poll-twice-past-the-end is a mandatory test for hand-rolled iterators
  (`iterators_none_stable`): the old `from_fn` iter was None-stable by
  construction; an engine rewrite regresses it silently, and nothing
  in-crate ever polls past None (for loops, collect, Drop all stop).

---

## 22. Deltas consumers must read through NAMES, not pos pairs (2026-10-15)

**The bug (2026-10-15, blockbimap chase — twice in one session).** the
back-pointer chase first read event pairs as addresses (`iter`) but
forward, which misidentified chained gather entries (fixed by backward
composition); the second attempt consumed raw POS pairs (`iter_pos`) to
skip the "translate then detranslate" round trip — sound algebra, and
wrong: `descending_keys_ascending_values` failed, because the reversal
tier RE-PARAMETERS the block's translator AFTER the event is recorded
(`maybe_reverse` runs at the end of `insert`, past `gather_slots`'s
record). the event's recorded positions are pre-flip; the emitter's
physical slots are mirrored; `value_at_pos(to_pos)` read a hole. the
round trip through name space is what re-anchors the read: names are
era-stable across the flip by construction (the flip emits zero deltas,
names follow the mirrored slots), so `get(to_addr)` under the CURRENT
translator finds the moved entry wherever the block now keeps it.

**Why it stands.** the deltas protocol's entire currency is names —
pos pairs are a recording-time frame, only valid while the recording
translator is still current. `a2p ∘ p2a` cancels only under a fixed
translator, and the block reserves the right to change it after
recording (the flip today; offset turns are deferred with the same
shape). "redundant" translation through the stable currency is the
correctness mechanism.

**The rules.**
- recorded POS pairs are a recording-time frame, valid only while the
  emitter's translator is unchanged since the record. events held across
  mutations MUST go through `iter()`'s names — era-stable by construction.
- the translator mutation class: the turn happens pre-record (inside
  `open`); the flip was the one POST-record site (end of insert) — moved
  to insert's TOP on 2026-10-15, so an event's positions stay valid from
  the record to the emitter's next mutation. `iter_pos` is therefore
  `pub(crate)`-legal for IMMEDIATE consumers (bimap's chase runs inside
  the same insert pair); the public surface stays the name stream.

**Resolution of the chase half (later same day).** the forward/backward
chase complexity died at the ENCODING level, not the consumer level:
`DeltaPairs::Gather` now walks the recorded slides' segments and emits
ONE pair per moved element carrying the FULL hop (crossing count,
`w − segment index`) instead of chaining k 1-step slides through
intermediate names — every pair is final, the chase is a single forward
pass, and first-match holders get one swap per element regardless of
hop distance. the emission order is load-bearing in the new scheme:
hop pairs OVERLAP (froms and tos share names when hop < run length), so
pairs walk away from the spot (right side descends, left ascends,
far segment first) — every `to` stays ahead of the remaining `from`s,
or a first-match holder retargets the same entry twice. the
era-stability half stands unchanged.

---

## 23. The cursor walk's inline chain must be forced end to end (2026-10-16)

**The regression (2026-10-16, MaskCursor — caught by the scan bench,
verified by objdump).** moving the tier cursors' walk state (slot/ghost/
direction/word cache) into mask's `MaskCursor` layered the walk:
`next [always] → advance → step_up`. with `advance` a plain `#[inline]`
hint, the chain grew past what the inliner forces through and the hot
scan loop called an OUTLINED `advance` — a real `call MaskCursor::advance`
in the disassembly, a call + cursor spill per step: cursor full-scans
0.6 → 1.6 ns/elem (2.5×), same signature as the documented `Cursor::next`
outlining (blockset's next doc, 2026-10-16 (a)). one trap on the way: a
perl substitution meant to promote `advance` to `#[inline(always)]`
silently didn't match (wrong indent), so a "we tried inline(always), no
change" bench ran unchanged code — the disassembly was what exposed it.

**The rules.**
- walk-path methods in the cursor machine are `#[inline(always)]` END TO
  END: the tier wrappers AND `MaskCursor`'s `slot`/`advance`/`retreat`.
  a hint anywhere in the chain leaves the whole chain one size-budget
  failure away from an outlined call in someone's hot loop.
- keep the FORCED bodies small: `advance(p)` takes the slot the caller
  already read from `slot()` — the internal state re-read both doubled
  state traffic and grew the body. `step_up`/`step_down` stay plain
  `#[inline]`: their crossing scans inline fine FROM an always-inlined
  caller.
- the word cache living in a `MaskCursor` field does NOT serialize the
  loop (the value-passed-word warning on `step_up` did not materialize)
  — the inlined loop keeps it in a register; numbers matched the flat
  cursor. the warning is about passing a POINTER to the cache, not
  about which struct owns the field.

---

## 24. Window-first sampling's `hi` bound is the filter (2026-10-19)

**The trap (skipset.rs, caught in design review before it shipped).**
`next_set` is distance-UNBOUNDED — asked for the first live slot at or
after a gridline, it walks as far as it must. the tempting cheap
contract is "tier0 slot j = first live ≥ 32j, no window filter": a
5-window hole would fill five slots with the SAME repeated handle, and
every slot would always be Some. it is WRONG, and not just wasteful:
the descent's range math is `hi = 32·first_gt`, sound ONLY because the
sample of window w is the first item OF that window — nothing in
[32·first_gt, pos(sample)) can hold k's answer. with the repeated
handle leaking backward into slot w, a query landing inside the gap
takes `first_gt = w` and `hi = 32w`, silently EXCLUDING its own valid
answer positions in [32w, pos(X)). the filter `first < window_lo +
STRIDE` is not an optimization on the sampling — it is the correctness
condition of the `hi` bound (the `lo` side is symmetric and free).

**The rules.**
- a tier0 sample must always be the first item STRICTLY INSIDE its
  window — the window bound is load-bearing, never "tidy up" it away.
- `None` means "the whole window is empty" and nothing else. that is
  what lets the IDK descent widen honestly (and only) across real
  gaps, and what made banking/turn-bands/gridline holes harmless to
  the tier (the 2026-10-19 gridline-sampling phantom-None class: banked
  hole runs straddle gridlines exactly where inserts are hottest, so
  phantom Nones clustered in the hot zone).
- when a range bound derives from a sample, re-derive its soundness
  whenever the sample's reach changes. the `bhi` clamp to cap
  (window_of) exists because width·STRIDE overshoots cap when cap isn't
  a multiple of 32 — insert_at's Safety contract says `to ≤ cap`.

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