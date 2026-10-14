# doa — Dense Ordered Arenas

The problem, stated plainly

A malloc-per-node tree is hostile to the three things a database index wants. Every node is a separate heap object, so traversal is a tour of scattered cache lines. Serialization means walking the whole structure and rebuilding the pointer graph on the way back — pointers into the heap are meaningless on disk. And every pointer is a full usize, eight bytes of address to name something that sits in a collection you could name in two.

The obvious fix — store the whole tree in a plain array — fixes all three: traversal becomes a linear scan of adjacent memory, serialization becomes writing the array out, and pointers shrink to 16 or 32 bits. But it buys you a new problem, and this problem is the entire crate: arrays are terrible at inserting in the middle. Inserting between elements 41 and 42 means either everyone after the gap moves up one — and now every pointer in the tree that names anything past the gap is stale — or you leave the array sparse, and "sparse" is a slippery slope back to a heap.

So the real question DOA answers is: how do I insert into the middle of an array without breaking any pointer, while keeping the array's order meaningful and its density high? Everything in the crate is a consequence of taking that question seriously.

The two conceptual moves

Move one: separate a thing's name from its shelf position. Think of it like a database: the addr is a logical row pointer — assigned once when a node is born, never changed — and the position is where those bytes actually sit. The two are linked by the translator, which is nothing more than a tiny, invertible arithmetic formula (four numbers: two offsets, a shift, a rotation) mapping names to slots and back. The point of this separation is that shelf positions can change en masse while names stay perfectly still. When the store needs to double its capacity, it interleaves a fresh empty slot after every element — a "spread" — and every element's slot index doubles. That would normally invalidate every pointer in the tree. Instead, the translator's shift knob drops by one, and every existing name now maps to its element's new slot. Nobody was told anything. Nothing was repointed. The pointers were stable by construction, because the mapping itself absorbed the change.

This is the move that makes serialization credible: names survive relocation, so a name is a durable thing you can write to disk.

There's a subtlety here that is genuinely hard to internalize, and it's worth saying slowly: the numeric order of addrs means nothing. They can wrap around the top of the integer range. They're names, not positions. The only thing that carries real order is the physical array — position 0 always holds the smallest element, the last position the largest. Every invariant in the crate is phrased over position order, and every translation trick is judged by exactly one criterion: did it preserve position order? Once you've absorbed that flip — "the array is the truth, the numbers are just labels" — most of the crate reads differently than it did before.

Move two: mutation reports corrections; everyone else applies them. When a slide shifts a run of five elements up by one, every parent pointer into those five elements is now wrong. The conventional design has the mutator hunt down and fix the pointers itself. DOA can't do that — it has no idea what your tree looks like inside, and that's deliberate. Instead, the mutator finishes its physical work and hands back a small, closed-form description of what it did: "these slots moved by this delta," or "this range doubled its indices." Anything in the world that holds addresses — the block's own metadata, a walker's saved position, the consumer's stack of ancestors, the consumer's node fields — receives that fixup and corrects every address it holds.

This is an inversion of control, and it's the crate's most important structural decision. It's the reason arbitrary consumer trees can plug into crate-owned mutation logic: the crate owns the choreography (what moves, in what order, with what corrections reported at what moment), and the consumer only ever implements one method — "given a correction, fix your stuff." The complexity doesn't disappear; it gets centralized into a handful of functions where it can be reasoned about once.

The three ways to make room

Every space problem in the crate reduces to three operations, in escalating order of disruption:

- Slide — the local fix. There's a None hole four slots away; rotate the intervening run toward it, and the hole lands next to where you want to insert. Cheap, touches only the run between you and the hole. Costs one fixup: the run moved.
- Spread/grow — the global fix. No hole nearby, so double the store and interleave a None between every pair of elements. Now every gap is an insert point. Touches the whole store, but costs no fixups to the tree's pointers — the translator absorbs it. This asymmetry (slides are local but require corrections; spreads are global but require none) drives a lot of the design's shape.
- Cleave — giving up on this block. Out of capacity, out of shift budget: cut the block in two. Done naively this invalidates every cross-block name; done as a rotation, the right half's elements land on interleaved slots with fresh holes between them, and the new block's translator just gets one different rotation parameter. The names survive even the death of the block they were born in.

Why trees, and why the walker

Here's the constraint that shapes the whole architecture: a slide's fixup has to reach its recipients. Somebody must know who points into the moved run. For an arbitrary bag of nodes, that's unanswerable without scanning the world — which is why the crate is trees-only. In a tree, the referers of any moved run are discoverable by traversal: they're parents, and parents are found by walking. But "walking" requires state — where am I, how did I get here — and that state itself holds addresses, which means the walker is also a fixup recipient. That's the third layer of the design: the walker isn't a convenience API over the block, it's the piece of machinery that makes middle-insert possible at all, and its state has to survive the very mutations it triggers.

The three-layer walker split follows from this. The consumer owns the innermost layer (what does your node look like, how do I read its children, set its pointers) because that's the one thing the crate cannot know. The crate owns the middle layer (what does "next" mean in preorder vs inorder) and the open surface — the outer layer of slot-opening choreography (walk to the anchor, find space, apply the slide with its fixups). Wiring is deliberately NOT a layer: the consumer drives node inserts, drains, and repoints via the block directly; the crate supplies open slots at ordering-correct positions and the fixups that keep held addresses true. The orderings themselves are deeper than traversal conventions: the ordering is the layout. Preorder says a parent sits immediately before its children; inorder says it sits at the boundary between its left and right halves. That placement determines where a slot must open relative to the tree, which determines the anchor arithmetic per ordering. Choosing an ordering is choosing your mutation costs.

The hardest concepts, ranked

I'd split them into three kinds — worldview shifts, choreography, and sharp edges — because they fail differently: the worldview ones make everything else unreadable until they click, the choreography ones are just genuinely intricate, and the sharp edges will hurt you if you touch them without respect.

1. Addr-as-name, pos-as-truth (worldview). The single biggest conceptual hurdle. The instinct is to treat addresses as positions — everything in computing trains you that way. Here they're opaque, possibly wrapping labels, and only the array's position order is real. Until this clicks, the translator looks like a pointless layer of indirection and every fixup looks optional. After it clicks, the translator looks like the whole point.

2. The fixup protocol and its ordering discipline (choreography + worldview). Not the mechanism — "apply this remap to your addresses" is easy — but the sequencing rules and why they're load-bearing: corrections must be applied to the walker's own state before it's used to walk; a two-slot reservation must have both slides computed before either moves, because you cannot walk a tree that is half-mutated to find the second one; the run of moved elements must have its parents corrected by a walk that happens before the physical slide it describes. These rules are not stylistic. Each one exists because the alternative is walking a tree in a transiently invalid state — reading a slot mid-relocation, or trusting an index that no longer means what it did.

3. The open surface's anchor discipline (choreography). An open walks to its anchor (a node's position, or a child's subtree edge), finds a None, and applies the slide with the run-parent fixup walk before the physical move — the crate's densest machinery. The consumer's split flows ride it: preorder Y opens before the split child's mid-child (inside X's old span, so Y's children visit after it), a root's NR opens before R with Y before R's mid-child, both atomically when they'd interfere. Getting the anchor wrong doesn't fail the open — it produces a valid-looking tree that walks out of order, which is why the tests check walk order against position monotonicity rather than trusting insertion success.

4. The in-order boundary (the subtlest single fact). In inorder, a parent sits at a gap index fixed by DEGREE — not by how many children it currently has. That's counterintuitive (why doesn't the boundary move as children arrive?) and it's fixed that way so the split of a full node never moves it. Follow the consequence: inserting a child left of the boundary shifts which gap is "the parent's" gap — the parent would have to hop over a subtree. In-order is binary-only since the open-surface refactor and binaries never split, so the crate carries no hop; a consumer that drives in-order beyond binary owns the hop itself (open_2 + swap, with the block-root fixup when the mover is the root). This is the crate's best illustration that ordering semantics and allocation mechanics are one subject, not two.

5. The walk-==-slot-order canary (sharp edge). An occupied-but-unwired slot (insert without wire) is invisible to every walk — no child entry names it — yet a slide moves it like any other `Some`, so the walk desynchronizes from slot order. The per-visit + endpoint asserts in the run-walk fixup are the tripwire that catches it at the moment of inconsistency. (The old reservation model — `MaybeUninit` write-places, pending-reservation drop UB — is gone: slots are plain `Option<T>`, values always initialized, the store hands out no write-places.)

6. The modes as workload bets (breadth, not depth). Uniform, Anchored, Pluripotent aren't three algorithms so much as three answers to "where will space be needed next?" — answered with different initial translator knobs, different store backends, and different find-space ladders. None is individually hard, but their interaction surface (which one pins the root implicitly, which one grows at the edges and compensates the translator instead of moving anything) is a lot of context to hold at once.

## The set consumer — sortedblock

`BlockSet<K, A>` (sortedblock/) is the height-1 consumer: a flat sorted SET
over the same substrate. No tree lives inside, so the machinery that exists
to find and correct a tree's referers disappears. The simplifications vs
treeblock, each load-bearing:

- **addr = the slot's canonical name (`p2a`), not a birth-stable row id.**
  Names are semi-stable: a rotation turn (growth) preserves every live name
  by construction — the element physically relocates to its own name's new
  slot — so turns are invisible to holders; a slide TRANSFERS names (the
  mover adopts the vacated slot's name). The feedback is therefore
  (from_addr, to_addr) name swaps; holders retarget refs. treeblock cannot
  do this: its consumers hold addrs across arbitrary mutations inside a
  tree it can't see, so names are assigned at birth and slides report
  pos-space fixups for the consumer's own structures to apply.
- **2-knob translator, fn-ptr per direction (`sortedblock/translator.rs`).**
  treeblock's shift knob exists to keep spreads fixup-free while address
  width grows; (offset, rotation) alone provably can't survive width growth
  + post-rotation adjustment (carries break the rotation) — but sortedblock's
  address width is FIXED and growth IS the rotation turn (`pos → 2·pos` mod
  A; the new slots mint the idle names rotation leaves between the
  spaced-out old ones). The 4-knob closure degenerates and 2 knobs close.
  direction is a translator knob now: reversed negates the position term
  (`p2a(p) = (offset − p) ror rotation`), the fn-ptr specialization keeps
  the negate off the translation path (BTB-predicted indirect call, as in
  treeblock's translator). Offset turns (window moves) are the
  unimplemented third move; rotation turns work under ANY offset and
  either direction now.
- **no walker, no `Fixable`, no anchor discipline.** `gather_slots` =
  fetch-distance gather: the nearest hole's distance sets the fetch (one
  hole per FREE_STRIDE=8 traveled — the further the look, the more Nones
  banked beside k); within a stride it's one plain slide (a single
  `ptr::copy`, source's bitwise duplicate uninit-by-mask — the mask is the
  only init oracle; O(1) mask update: nearest hole ⇒ the run is fully
  live). farther: `fetch_holes` pre-reads the chain, `compact_gather`
  applies it as one segment pass — each member crosses exactly once, far
  segments copy first so no live source is overwritten. No run-walk fixup
  choreography, no ancestry.
- **Deltas = one compact event.** `enum Delta { NoneSlide{from,to}, Gather,
  Pairs }` — the common contiguous slide records the hole's `from → to`
  (elements between shift one step toward `from`; `from == to` = no-op
  direct place); a multi-fetch records `Gather(Vec<NoneSlide>)` — the
  slide chain, nearest hole first, each slide in post-prior-slides
  coordinates (identical to its pre-read position: a hole's slot is never
  touched before its own slide; a two-sided gather is two chains' worth,
  side told by `from > to`). `Deltas::iter` lazily expands to the same
  swap-cascade pairs (hole-adjacent first) the consumer always saw.
  `Pairs(Vec)` still waits for non-run shapes (bulk). Recording a slide
  costs one write; the old per-pair Vec pushes cost 2× the memmove they
  described (bench_notes).
- **occupancy = `Mask` (`src/mask.rs`)** — Vec<u64> words, word-at-a-time
  scans (`next_set`/`next_clear`/`prev_clear`/`nearest_clear`; the find
  ladder's primitives; all four carry a dense fast path — the query
  position IS the answer when its bit matches — and `#[inline]`), the
  `SetBits`/`SetBitsRev` iteration engines (in-place bit test against the
  cached word, `pos` as the state so yields are free; holes advance one
  bit, whole-empty word tails skip; exhaustion must zero the cached word
  at EVERY site or a re-poll yields out-of-range positions — subtle_bugs
  §21), `rotate_mask` (the turn's mask half: interleave + one band
  ripple — upper-half names land odd; the reversed band ripples DOWN
  over the suffix `(offset, cap)` and can cross word boundaries, so the
  fall carries word by word) + `reverse` (the reversal's mirror).
  `nearest_clear` excludes its center by contract (searches ±1;
  callers pass a position they occupy) — `gather_slots` checks
  spot-is-hole itself first, because the insert SPOT is usually a hole
  (the bsearch converges lo to k's gap), not a live slot. bitvec was
  dropped — miri-unclean (integer-to-ptr provenance in its own
  internals), and the word form is what makes the scans fast anyway.
- **reversal = two mirrors + a negated translator, zero deltas.** the
  array mirrors (`data.reverse` + `mask.reverse`), the translator
  re-parameterizes (`p2a_new(p) = p2a_old(cap−1−p)` — exactly `offset →
  cap−1−offset` with the direction flipped), names follow the mirrored
  slots BY CONSTRUCTION — a pending deltas event survives the flip
  untouched (it speaks names; names didn't change). trigger:
  mean insert position (`pos_sum/len`, doubling at turns, reflecting at
  flips) below len/8 past a 4·stride warmup — the all-time doubled sum
  is an intentional estimator (each doubling era votes once: biases
  develop early, stickier later). at len/4 a uniform stream flips 4×
  spuriously (bench_notes); a real front stream's mean sits ≈ 0 either
  way. see subtle_bugs §20 for the reversal turn's band + the
  `offset < cap` rule offset turns must respect.

Deferred: the `try_alloc`/`try_insert` split + arena-level occupancy
decisions (`insert`'s ladder owns the in-block heuristics now), offset
turns (window moves — mask + data halves both handle any offset; the move
op itself remains), `from_vec`, convenience getters. Benchmarks
and phase breakdown in `notes/bench_notes.md`.


## Style
- comment at every call that alters the walker's position or the node it points
  to (`descend`/`ascend`/`set_position`/`swap_current`, raw `block_mut().swap`,
  the post-slide state restores) — say what the walker names afterward. flows
  read as choreography; silent repositioning is where the bugs hide.

## Workflow

- sessions start from a clean tree: the previous session ends with a commit. remind the user before making code edits if the git diff is dirty. 
- session-end routine, in order: run `skeletonize.py` (regenerates the doc/
  outlines), spawn a **fresh** review agent (not a fork) on the session's diff —
  it reads this file and subtle_bugs.md first so it doesn't re-flag intentional
  choices — apply findings, update this file + subtle_bugs.md (merge in place,
  prune stale entries), re-run `skeletonize.py` if src changed, and leave the
  tree ready for commit — the user commits.
- review priority: correctness (bugs, invariant breaks, do-not-revive violations) >
  cheap cleanups > doc-record > perf. unbenchmarked perf suggestions are reported,
  never auto-applied.
- "defer" means written into this file's Status or into subtle_bugs.md — never
  "remembered".
- do not update the claude.toml until the session is ending. 

# Style
- doc comments are the single source of truth for item outlines - keep them minimal, don't make them a summary, just purpose, invariants, and panics. 
- single line comments may be introduced in long functions to concisely explain what a block of code does
- a comment must never be longer than the source it applies to. 
- function names and variables should be concise but explanatory - avoid arbitrary letters and abbreviations.
- module item order: `use` → `mod` → structs → types → traits → impls → macro
  invocations. a macro is defined where its output class lives — one generating
  structs sits with the structs, one generating impls with the impls.
- each file's `//!` header carries its purpose + invariants. this file keeps the conceptual
  map only — item inventories live in doc/ and are generated, never hand-edited.

## Files (lowest level → highest)

Item inventories live in `doc/<name>.md` — generated skeletons (fenced rust,
`///L####` tags jump to source). This section is the conceptual map only; the
files' `//!` headers restate purpose + invariants next to the code.

src/ is a shared substrate + two consumers:

substrate:
- `lib.rs` — module wiring + the ordering/side vocabulary
  (`RootPos`/`Order`/`Ordering`/`Rel`); re-exports keep old paths stable
  (`doa::Fixup`, `doa::treeblock::{search, walker, TreeBlock}`).
- `index.rs` — numeric trait ladder (`Num`/`UnsignedNum`/`Addr`) + `Pos` +
  type-level const facts underpinning all address math.
- `mask.rs` — dense occupancy bitmap (see the sortedblock section).
- `translator.rs` — the 4-knob treeblock translator, fn-ptr-specialized over
  zero/nonzero params; the one hard rule is position order (pos 0 = min, pos
  len−1 = max). sortedblock's 2-knob translator lives in its own module (see
  `sortedblock/translator.rs`).

consumers:
- `treeblock/` — ALL the tree machinery: `blocks` (Block + BlockTrait/
  BlockOps + the three modes), `walker` (Node contract + walker layers +
  open surface), `treeblock` (TreeBlock marker + constructors), `metadata`
  (the fixup protocol + tree-state types; Pos lives in index now), `store`
  (slot backends + slide/gather/grow primitives), unwired `block_cursor`/
  `leafblock`/`inline_leafblock`, and the tests (live `tests/walker.rs`,
  unwired `tests/store.rs`/`tests/block.rs`).
- `sortedblock/` — the set tier: `mod.rs` (contract), `store.rs` (COPY of
  treeblock's store evolving the pairs feedback protocol — `open_slot`
  emits swap-cascade `(from, to)` pairs; gather/bulk still plan-returning),
  `translator.rs` (the 2-knob translator, fn-ptr per direction + the
  reversal/turn maps), `sortedblock.rs` (`BlockSet` + `Deltas` + the
  fetch-gather ladder + the reversal trigger + the differential `Iter`
  over SetBits/SetBitsRev).
- `benches/` — `sorted_find_slot.rs` (mask find vs store scan),
  `sorted_insert.rs` (append/prepend/random/runs × insert/lookup/seqread
  vs sorted-vec + BTreeSet, 32-byte items, size via CLI arg — default
  10k, u32 addrs — with a 3s wall budget per structure×workload cell and
  partial-rep reporting; findings + phase breakdown in
  `notes/bench_notes.md`).

notes: `devlog.md` (design log), `subtle_bugs.md` (solved nuances + their
rules), `bench_notes.md`.

archive: `src/archive/` + `examples/old_btree/` (the live tree consumer is
`examples/btree.rs`).

## Testing

`treeblock/tests/walker.rs` is LIVE on the open surface: a preorder B+ consumer (Vec
nodes, DEGREE 4 — consumer-driven splits) with 240-key torture (ascending/
descending/stride) + hand-assembled anchors + held-addr fixup delivery, and an
in-order DEGREE=2 binary consumer for rotate_left/rotate_right (root / leaf
riser / mid) — validated by structural checks (separator re-derivation from
child mins, leaf order, reachable == occupied) and walk order vs strictly
increasing positions; miri-clean.

`treeblock/tests/store.rs` stays UNWIRED (written against the addr/pos surface;
re-enable after porting) — reference-model torture + exhaustive slide/find/
find-2 matrices for both backends, spread/split/pop coverage, drop accounting,
contract panics. `block.rs` stays unwired (pre-refactor API — needs adaptation).

sortedblock is inline `#[cfg(test)]`: mask (exhaustive scans vs naive
comparison at every start position; `rotate_mask` vs naive models at every
offset — forward prefix/suffix/whole/empty bands, reversed down-band with
word-edge sweeps; `reverse` mirrors + double-reversal identity) +
translator (`names_survive_reversal_and_turns`: round-trips, the
name-mirror, double-reversal identity, and the reversed turn map
`q = 2p − [name ∈ upper half]` — name-stable, monotone, descend-safe —
over every reachable (rotation, offset<cap)) + BlockSet
(ascending/descending/stride torture with a holder mirror applying
translated deltas; `mixed_ops_torture` — clustered + descending-bait
streams with random removals, every holder name verified every op, order
valid in either direction post-flip; the reversal fires in `descending`;
gather banking by distance; front-insert cascades; pending-deltas panic;
dup rejection; remove/reinsert; NoneSlide expansion).

Run targeted: `cargo test -p doa --lib sortedblock` / `mask` /
`treeblock::tests::walker::tests`; uninit/leaks: `cargo +nightly miri test -p
doa --lib <filter>` (slow — recompiles + interprets; sortedblock is
miri-CLEAN — run via the miri driver directly on this nightly: cargo-miri
drops a path segment resolving the workspace and fails to launch its
compiled test binary, see devlog).
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Status

Walker-surface refactor landed (2026-09-19). The crate no longer drives tree
mutations — it supplies OPEN SLOTS at ordering-correct anchors; all wiring
(node inserts, drains, reparenting, root promotion) is consumer-side, against
the block directly. The pieces:

- consumer mask flattened: the walker IS its own state (`NodeCursor` reads +
  consumer-impl'd `descend`; `NodeWalkerMut: Fixable` + `Snapshot`/save/load/
  set_position (internal machinery — proposed for deletion 2026-09-19 and
  KEPT after review: save/load is fixup's correctness instrument, the run
  walk rewrites entries to post-slide addrs so walking back is unsound,
  subtle_bugs §5; the per-slide clone is an optimization target, not a
  subtraction) + set_child/clear_child/set_parent (both kept by decision));
  `lookup -> Option<ChildPos>` (None = descent terminates — the consumer's
  equal policy); `children() -> (ChildPos, A)` pairs, DoubleEnded+ExactSize.
  Died: the `State` assoc type + accessors + `parts`/`parts_mut`, `has_space`,
  `current_mut`, walker-level insert/remove_child, `SplittableNode`,
  `Node::Payload`, the `DEGREE >= 3` bound (binary rotations are legal).
- opens consume the walker (`open_here`/`open_child`/`open_2_child`;
  preorder adds `open_parent`/`open_parent_child`; in-order adds
  `rotate_left`/`rotate_right` — physically free, walker survives). Opens
  return `(slot(s), OpenFixups)` — consumers holding addrs across an open
  apply the returned fixups (held addrs never survive an open uncorrected).
  `Suggested`/suggest fns/`hop_current`/postorder impls/`TreeWalkMut`/
  `SplitWalkHelper`/`SplitTreeWalker` deleted; the sealed `TreeWalkHelper`
  (slide engine, open engines, `Anchor`) is `pub(crate)`.
- block surface: `TreeBlock::set_root` + `split_block(self, l, r) ->
  (Self, Self, N, usize)` (popped root + prefix split boundary; body `todo!` —
  arena tier), `BlockOps::pin_pos` (Anchored's pin, deduped out of its
  find/slide impls) + `try_find_slot`/`try_find_2_slots` (scan-only probes —
  no grow escalations; the split-vs-grow decision's space test).
- find ladder fix (found by the 240-key ascending torture): append-heavy
  growth densifies the store edge past the fixed `BIT_WIDTH` budget while
  mid-span holes remain — forced spreads ran to shift-exhaustion (len 65536,
  occupied 142). Uniform + Anchored ladders gained a full-len scan rung
  between the budgeted scan and the spread.
- `examples/btree.rs` rewritten: preemptive consumer-driven splits (full
  node on the descent path splits before anything descends into it — the
  parent of any split target had room by construction), inlined split flows
  (the opens consume the walker, releasing the block borrow for drain/wire),
  held-addr + returned-fixup discipline. Runs green: 100 keys, multi root
  promotions, all anchor kinds, in-run + out-of-run slides.
- `src/tests/walker.rs` live on the new surface (see Testing) — miri-clean.
  The old split-driver/hop/postorder torture died with its machinery.

Second session (2026-09-19, same day) — the three agreed directions landed:
- **rotations → the stand-on-the-riser contract** (agreed direction,
  2026-09-19): the
  walker starts ON the riser, all writes via up-navigation over the ancestry
  stack, ends ON the riser — with the ancestry popped ONE level, because the
  riser ROSE: "ends where it started" is position-only, and the end state
  must be TREE-TRUTH (the descent-history entry naming P via slot 0 is stale
  post-rotation). The final pop is `ascend` + `set_position` — a state-only
  restore over the fully-consistent post-rotation tree; what died is the old
  MID-CHOREOGRAPHY reach (walking to the riser through a half-rotated tree).
  Root/mid-tree uniform; root case ends with an empty stack. The original
  pseudocode omitted the riser's own stored-parent-field write — kept (the
  old code's third `set_parent`; parent-free shapes no-op it).
- **§4b partial**: `set_parent -> Option<B::A>` — the overwritten old parent
  addr, `None` for parent-free shapes; crate call sites discard it (what a
  walker does with it is consumer territory — journaling `save`/`load` or
  ascend-by-old; `Some` returns are unexercised, no parent-field consumer
  exists). `Ancestry` is inline-backed (8 entries + spill, Index/IndexMut):
  fixup's per-slide snapshot/restore is a fixed-size copy below depth 8. The
  clone itself stays — irreducible for stack walkers (§5: popped anchor-path
  entries can't be re-derived mid-fixup); the heap alloc was the cost.
- **gather machinery (v1)**: `GatherSlide` (metadata.rs) — the N-None gather
  as a closed-form CROSSING-COUNT fixup (`delta(pos) = ±#{holes beyond
  pos}` — NOT sequential `NoneSlide`s: overlapping runs go stale
  mid-application). `Store::find_n_slots` (rel-side nearest-n, pin-clamped) +
  per-store `gather_none` — one directional compaction pass (After descends
  moving members up, Before ascends moving them down; each slot's own
  iteration precedes any write into it), every member Some crosses the run
  exactly once. `BlockOps::find_n_slots` ladder (budgeted → full-len →
  spread+rescan → exhaustion) + `gather_none` (fixes BlockData). Walker:
  `fixup_gather` (one single-direction run walk, per-visit + endpoint
  canaries, snapshot/restore), `apply_gather` (`reparent_range` =
  reparent_run's body generalized to explicit Some-dense bounds),
  `open_n_at` + surface `open_n_here`/`open_n_child` on both orderings.
  `OpenFixups.gather` (no longer Copy); `Fixable::gather_fix` (by-ref).
  v1 scope: holes all on the open's rel side (one walk segment per gather);
  Pluripotent gets the default ladder (no edge-grow for n>1); Uniform lacks
  the proactive 3/4-spread rung.
- tests: the rotation tests reworked (descend to the riser first; end
  position == start + end-STATE truth asserted — is_root at the root, the
  true parent mid-tree); `pre_open_n_after` (3-slot gather, held-addr
  delivery), `pre_open_n_before` (Before child edge, prev() walk),
  `pre_open_n_before_adjacent` (hand-built store — the anchor-adjacent slot
  OCCUPIED, the member interval extends to anchor−1),
  `pre_open_n_deep` (2*h-slot burst — two fresh (h-1)-deep internal chains +
  leaves into a multi-level map; member internals with moved parents).
  12 tests, miri-clean. The in-order `open_n_*` surface is untestable against
  the binary consumer: any 2-node burst needs a slot on each side of the
  parent gap, or produces leftless-right chains (unwalkable) — it waits for a
  DEGREE>2 in-order consumer.
  Fresh-review findings (all fixed pre-commit): the Before member interval
  stopped at anchor−1 instead of the anchor (adjacent-occupied case tripped
  the canary / skipped a member's entry rewrite), the rotation end state kept
  the stale descent entry (is_root/parent lied), and the Before reparent
  range underflowed on the empty-member edge.

Deferred: `split_block` body; store tests port (still unwired — now also the
gather matrices). Agreed, not yet implemented: the §4b walk-back USAGE
(ascend-by-old for parent-field walkers — the return is in place, no
consumer exercises it).

Known latent flaws: reparent_run's
`set_position` jumps (unexercised), the unary-internal-riser panic,
packed-ChildPos nav breaking on sparse-addressed nodes, the STORES_PARENTS
walker hazard class (unvalidated — no such consumer exists; also the
unexercised `set_parent` Some-return),
children()-sortedness unchecked.

Not designed/wired: sparse-ChildPos nav (children()-based sibling walks —
one-kid in-order nodes are reference-walked in tests, not TreeWalk'd);
deletion rebalancing/merges; serialization; `leafblock`/`inline_leafblock`;
`block.rs` test adaptation.

Third stretch (2026-10-06 → 10-12) — module split + BlockSet insert live
(one commit at this session's end):
- **module split**: src/ = substrate (`index` + `Pos`, `translator`, `lib`
  wiring) + `treeblock/` (all tree machinery, byte-identical) +
  `sortedblock/`. re-exports keep old paths stable. notes/ moved under
  doa.
- sortedblock/store.rs pairs protocol (10-06): `open_slot` fused
  find+slide+record emitting swap-cascade `(from, to)` pairs — the
  treeblock fixup contract doesn't apply (set holders want plain pairs);
  gather/bulk still plan-returning. `benches/sorted_find_slot.rs`: mask
  find 1.28×/1.45× at 80% occupancy (see notes/bench_notes.md).
- (10-12) `BlockSet` insert basics: bsearch over live slots (Ok = dup,
  Err = insertion spot — conventional semantics; holes skipped forward,
  sortedness proves everything before the probe known), `try_alloc`
  (nearest hole either side via `Mask::nearest_clear`, fails clean),
  in-place rotation-turn grow (`pos → 2·pos` descending pass +
  `Mask::spread_double`; false while offset ≠ 0), slide = one
  `ptr::copy` + O(1) mask, `Delta::NoneSlide` compact event with lazy
  cascade expansion, `Drop` via `assume_init_drop`, cap 0→4 vec-like
  first allocation. 10k shuffled inserts: 386 → 87.5 ns (ladder + phase
  breakdown in bench_notes; the per-pair delta Vec cost 2× its memmove —
  the compact event killed it). bitvec dropped (miri-unclean in its own
  internals) for `src/mask.rs`.
- sortedblock translator settled as the 2-knob degeneration (the devlog's
  adaptive-translator analysis, lines 21–119, is the reasoning; the
  sortedblock section above is the map).

Fourth session (2026-10-12 → 10-13) — deltas safety, biases → the unified
ladder (one commit at this session's end):
- **deltas safety**: `Delta::Empty` is the explicit sentinel (pending ⇔
  event ≠ Empty); `insert` panics on pending deltas instead of clearing;
  `Deltas::clear` public. caller protocol: apply, then clear.
- **biases → unified ladder**: started as `InsertBias` → (Direction, Growth)
  split (Desc = reversed blocks: bsearch operand flip via `locate`, reverse
  `iter`), then Growth DELETED when the ladder unified — the same rungs for
  every block: strategy-free tail fast path (`spot`), edge rule (`spot ==
  cap` → `push_back` — Vec's own doubling carries cap; hole every BIT_WIDTH
  slots by tail-run cadence), then `open`'s rungs: BIT_WIDTH probe →
  back-open (push_slot + slide when the tail run is within budget) → dense
  turn (len ≥ cap − cap/5) → local turn (len ≥ cap − cap/3 AND nearest
  hole > 4·budget — spreads never leave the block below ⅓) → fluke wide
  slide → mask-full grow. `push_slot` is the single growth primitive
  (grow_rotate is a layout change, not a capacity change — subtle_bugs
  §18). `insert` split: `spot`/`insert_first`/`push_back`/`open`/`place`.
- **rotate_mask(offset, midpoint)** replaces `spread_double`: plain
  interleave + one masked ripple over the wrap band (prefix/suffix/
  whole/empty); `grow_rotate` moves via the translator composition
  `new.a2p(old.p2a(p))` (names by construction), refuses at cap >
  MIDPOINT. the +1 band is the recycled rol bit (subtle_bugs §19).
- bench (32B items): sequential 8 both columns after the review's cadence
  fix (the hole goes BELOW k — appends can't consume it; the pre-fix
  trailing hole was eaten every append, leaving the block dense so every
  push_back paid a full-mask prev_clear + doubled push: 64), random 99.3
  lazy → 88.7 preemptive → 87.0 unified, wrong-direction column (descending
  into Asc) 692 — irreducible, the fix is Direction. ladder + findings in
  bench_notes.

Fifth session (2026-10-13) — fetch-gather + the reversal tier (one commit
at this session's end):
- **the unified ladder died for fetch-distance**: `FREE_STRIDE = 8` replaces
  budget; no cadence holes (push_back dense), no 80%/⅔/4·budget turn
  rungs. `open` = preemptive turn past 7/8 when the insert sits >
  FREE_STRIDE from the edge (devlog line 199's rule) → `gather_slots` →
  mask-full: one tail push within FREE_STRIDE of the edge, else turn /
  refused tail slot. the whole prior ladder's probe/back-open/turn-rungs
  are gone.
- **`Delta::Gather(Vec<NoneSlide>)`**: multi-fetch chains as a Vec of
  hole-journeys, lazily expanded to the same cascades holders always saw;
  the single-slide fast path stays the alloc-free `NoneSlide`.
- **reversal tier**: `sortedblock/translator.rs` extracted (fn-ptr per
  direction — the negate never touches the translation path);
  `BlockSet::reverse` = data+mask mirror + `offset → cap−1−offset` +
  pos_sum reflection, ZERO deltas (names follow the mirrored slots —
  pending events survive). trigger `pos_sum·8 < len²` past a 4·stride
  warmup (len/4 flipped pure-random 4×; len/8 costs a real front stream
  nothing — its mean sits ≈ 0). `Direction`/`InsertBias` died; the block
  reads direction off the translator.
- **turns under ANY offset + reversal** (the `offset ≠ 0` refusal deleted):
  the reversed turn's band is the pos-suffix `(offset, cap)` landing
  `2p−1` — a DOWN-ripple, which crosses word boundaries (bit-0 element
  falls to the word below's bit 63; the fall carries word by word).
  `rotate_mask(offset, midpoint, reversed)`. reversed blocks turn again —
  a flip is no longer a degenerate tail-push trap. the reversed turn
  requires `offset < cap` (name 0's slot — holds in every reachable
  state; offset turns must respect it): subtle_bugs §20.
- **position/bsearch**: direction dispatch outside the loop, monomorphized
  comparator per arm — no order branch in the hot loop.
- bench (32B, 10k, best): append 8.5, prepend 8.5 (692 wrong-direction →
  74 gather-banked → 8.5 flipped), random 99 ≈ parity with the old
  ladder on the same machine-day. phase table + the chained-gather
  quadratic (1704) in bench_notes. miri-clean via the driver.
- fresh-review: no correctness bugs; promoted the mixed-op randomized
  torture inline, fixed doc drift (Gather coordinates frame, descending
  comment, mod.rs invariant line), wrapping-mul hygiene in the trigger,
  deleted the dead `back` field.

Sixth session (2026-10-14) — bench scale-out, seqread engine, lookup
fast paths (one commit at this session's end):
- **bench rework**: `sorted_insert.rs` takes a size CLI arg (default 10k;
  u32 addrs — u16's name space caps at 64k), a 3s wall budget per
  structure×workload cell (slow cells finish one partial rep, reported
  over the completed prefix, `[partial done]`-marked — SortedSet's O(n²)
  columns stay benchable at 1M), a runs workload (chunks of 100
  sequential keys, half reversed to descending runs, chunk order
  shuffled), seqread + lookup columns, and the `sorted-vec` crate as a
  third column (dev-dep). 1M findings: the cache cliff is measured now
  (random insert 96→487, lookup 68→330), runs is the scale-proof
  workload (132 at 1M), seqread scale-invariant, BTreeSet's lookup edge
  — since inverted by the fast paths below.
- **seqread engine, 3 rounds + inline** (5× end to end, 1.98→0.4
  ns/elem): the `from_fn`+`next_set`/`prev_set` iter died for the
  differential `Iter` enum (Asc/Desc picked once) over word-stepping
  engines; Desc 2.81→1.41 (killing `prev_set`'s keep-mask loop was the
  biggest algorithmic fix); then the in-place bit test (cached word, `pos`
  as the state — yields free, the position rebuild was the hidden
  cost); rotation-mask and tz-jump variants measured worse; then
  `#[inline]` on the per-element `next`s — 1.4→0.4, the round that
  mattered most (cross-crate call overhead was the dominant term; check
  it FIRST, tune loop bodies second). masked scan now 0.4-0.8 ns/elem
  cache-resident (~1.4 cycles), within 2× of bare memory at both
  scales.
- **bsearch fast paths** (lookup 68→57 at 10k, 328→256 at 1M; insert
  rides it): `next_set` returns the probe position directly when its bit
  is set — the ~90% case — skipping the shift/tzcnt/rebuild. the other
  three scans got the same fast path + `#[inline]` (flat on insert:
  hole-finders' queries are dense by construction — the payoff case
  there is the 10% case; kept for API uniformity at zero measured cost).
- **`nearest_clear` excludes its center** (contract; callers pass a
  position they occupy): initially broke insert — the insert SPOT is
  usually a HOLE (the bsearch converges lo to k's gap), and
  `gather_slots` relied on nearest_clear returning the spot for that
  case; fixed by an explicit spot-is-hole mask test at the top of
  `gather_slots` (the check the old scan did implicitly).
- **batched lookup: built, measured, DROPPED.** `lookup_batch<const B>`
  went through five designs (median-partition descent → level-sync
  frontier → serial+running-lo → plain sorted → evens/odds two-phase
  with an adaptive span rule). spread keys are parity-with-singles at
  every B and both scales (singles' OOO overlap + shared probe pyramid
  are near the floor; the descent saves only ~log2(B)−2 probes);
  clustered batches win 3-13× — but the probe-path sharing does most of
  that WITHOUT any API (a globally sorted probe stream through plain
  `contains` is the 2.4×/1.7× win). not worth the complexity — do not
  revive; full analysis in bench_notes (2026-10-14 sections).
- fresh-review: one real bug found and fixed (iterator re-poll
  instability — stale cached word after exhaustion yields OOB
  positions; subtle_bugs §21, `iterators_none_stable` regression test);
  sorted-vec moved to dev-deps; doc drift updated.


## Future Work

- sortedblock: `try_insert`/`try_alloc` split + arena-level occupancy
  decisions; threshold tuning (FREE_STRIDE, the 7/8 turn line, the len/8
  reversal trigger are unbenchmarked knobs); holder-applying bench variant
  — the cascade savings (turns/flips are zero-churn) aren't measured by a
  deltas-clearing bench.
- sortedblock: the 7/8 preemptive turn fires before checking whether the
  spot is already open — delete-max + reinsert-max pays a full turn for a
  free slot (report-only finding; a nearest-hole-within-stride guard is
  the candidate, unbenched).
- sortedblock: offset turns (window moves — mask + data halves handle any
  offset now; the move op remains, and it must respect `offset < cap` or
  generalize the reversed band — subtle_bugs §20),
  mixed-side/multi-event deltas (`Delta::Pairs` awaits bulk),
  `from_vec`, convenience getters.
- sortedblock: find-bench density sweep (90–97%),
  pairs-based bulk open in store.rs (gather → pairs).
- fixup walk-back usage (§4b remainder) — ascend-by-old-value for
  parent-field walkers, over `set_parent`'s `Some(old)` return (in place,
  unexercised); needs a STORES_PARENTS consumer to validate at all.
- mixed-side gathers — holes from both sides of the anchor (the member
  interval splits in two, one walk per side, anchor shifts by the
  opposite-side crossings); v1 is rel-side-only.
- store tests port (still unwired) + gather matrices for `find_n_slots`/
  `gather_none` — the 240-key reference-model torture.
- `split_block` body — the cleave/rotate choreography (blocks.rs's per-mode
  `cleave*` are the starting points), arena handoff.
- canary negative test — occupy-in-run-without-wire → expect the panic.
- extend the btree consumer (deletes with leaf removal, underfull merges).
- `keys()` iter hook + ordered iteration (`IntoIterator` on the walker).
- sparse-ChildPos nav — children()-based sibling walks for one-kid/sparse nodes.
- arena tier — subtrees & forwarding (block_id roots), ordering across splits;
  graduation — pluripotent → concrete strategy at len == the `Half` cap.
- `Fixup` relevance-check optimization (elide unnecessary runtime checks).
- trie integration.
