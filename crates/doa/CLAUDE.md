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

## The set consumer — blockset

`BlockSet<K, A>` (blockset.rs) is the height-1 consumer: a flat sorted SET
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
- **2-knob translator, fn-ptr per direction (`src/translator.rs`).**
  treeblock's shift knob exists to keep spreads fixup-free while address
  width grows; (offset, rotation) alone provably can't survive width growth
  + post-rotation adjustment (carries break the rotation) — but blockset's
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
  side told by `from > to`). `Deltas::iter` expands: plain slides to 1-step
  cascades (hole-adjacent first), gathers to ONE pair per moved element
  carrying the FULL hop (crossing count, `w − segment index` — walked
  away-from-spot so every `to` stays ahead of the remaining `from`s for
  first-match holders; hop pairs overlap, order is load-bearing —
  subtle_bugs §22). `Pairs(Vec)` still waits for non-run shapes (bulk).
  Recording a slide costs one write; the old per-pair Vec pushes cost 2×
  the memmove they described (session_history).
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
  spuriously (session_history); a real front stream's mean sits ≈ 0 either
  way. see subtle_bugs §20 for the reversal turn's band + the
  `offset < cap` rule offset turns must respect. the check runs at
  insert's TOP (pre-record, before `spot`) — the trigger sees the
  pre-insert signal, and no translator mutation follows the event record
  inside insert: pos-space deltas consumers (`iter_pos`, bimap's chase)
  rely on that (subtle_bugs §22).

Deferred: the `try_alloc`/`try_insert` split + arena-level occupancy
decisions (`insert`'s ladder owns the in-block heuristics now), offset
turns (window moves — mask + data halves both handle any offset; the move
op itself remains), `from_vec`, convenience getters. Benchmarks
and phase breakdown in `notes/session_history.md`.


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
  choices — apply findings, append the session's entry to
  notes/session_history.md (format contract at the file's top), update this
  file + subtle_bugs.md (merge in place, prune stale entries), re-run
  `skeletonize.py` if src changed, and leave the tree ready for commit — the
  user commits.
- review priority: correctness (bugs, invariant breaks, do-not-revive violations) >
  cheap cleanups > doc-record > perf. unbenchmarked perf suggestions are reported,
  never auto-applied.
- "defer" means written into this file's Future Work or into subtle_bugs.md — never
  "remembered".
- do not update the claude.toml until the session is ending. 

# Style
- doc comments are the single source of truth for item outlines - keep them
  minimal: 1-2 lines, what the item is FOR plus anything non-obvious about
  its contract (assumptions, guarantees, panics, None/return semantics).
  they never describe HOW the function works - mechanism lives as
  single-line annotations at the site doing it (rationale at the branch
  that needs it, safety notes at the loop that depends on them). variant
  and field docs carry their own semantics. SAFETY blocks and `//!` module
  headers are exempt - they hold the full contract.
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

src/ is a shared substrate + the set/map tiers (primary surface, flat) +
the tree consumer:

substrate:
- `lib.rs` — module wiring + the ordering/side vocabulary
  (`RootPos`/`Order`/`Ordering`/`Rel`); re-exports keep old paths stable
  (`doa::Fixup`, `doa::treeblock::{search, walker, TreeBlock}`).
- `index.rs` — numeric trait ladder (`Num`/`UnsignedNum`/`Addr`) + `Pos` +
  type-level const facts underpinning all address math.
- `mask.rs` — dense occupancy bitmap (see the blockset section).
- `translator.rs` — the set/map tier's 2-knob translator, fn-ptr per
  direction + the reversal/turn maps; the one hard rule is position order
  (pos 0 = min, pos len−1 = max).

tiers:
- `blockset.rs` — the set tier: `BlockSet` + `Deltas` + the fetch-gather
  ladder + the reversal trigger + the differential `Iter` over
  SetBits/SetBitsRev; `find`/`contains` fast-path beyond either extreme
  (two compares, no bsearch — sequential probes land past the tail).
  `Deltas`' event recording (`record_none_slide`/
  `record_gather`, `is_pending`), `NoneSlide` (the deltas-protocol link),
  and `FREE_STRIDE`/`L2_BYTES` are `pub(crate)` — blockmap shares them.
  (the old blockset store copy — treeblock's store evolved toward pairs
  feedback — was deleted 2026-10-15: unused by the tier itself; the
  find bench now runs against treeblock's store.)
- `blockmap.rs` — the map tier: `BlockMap<K, V, A>` — the blockset
  substrate with a value riding beside every key: TWO parallel
  `MaybeUninit` vecs (keys/vals) under ONE shared mask, bit ⇔ the pair
  live; every slide/gather/turn/reversal relocates the pair in lockstep,
  the mask half of turn/reversal runs once. borrows blockset's `Deltas`
  + the 2-knob translator unchanged — value slots are nameless, a name
  finds the pair. API beyond the set surface: `get(addr) -> (&K, &V)`,
  `value`/`value_mut` by key, `set_value` by addr (drop-safe overwrite),
  `remove_key`, `iter` over `(&K, &V)`; `find` carries the same
  extremes fast path as blockset's.
- `blockbimap.rs` — the bijective tier: `BlockBiMap<K, V, A>` — two lockstep
  BlockMaps, each holding the OTHER's names as its values (both orders native,
  no projection ordering; every pair one hop from either side). insert
  churns BOTH name spaces — `BiDeltas` carries both `Deltas` streams; the
  maps consume each other's events by the BACK-POINTER CHASE (`chase`): a
  delta pair names the emitter's moved entry, and that entry's own value
  addresses the one partner slot whose held name went stale — O(1) per pair,
  no search. gather pairs carry the full hop, so every pair is final — the
  chase is a single forward pass in POS space (`iter_pos` + reads at pos:
  the flip fires pre-record, so the emitter's translator is unchanged from
  record to chase; events held across mutations go through `iter`'s
  era-stable names — subtle_bugs §22). the fresh entry's dummy value (A::ZERO) is never chased before
  its patch: only its own partner points at it, and a placed entry never
  moves. no value mutation API — V is a values-map KEY, mutation would
  break its order. dup detection lives IN the maps' inserts (no pre-check
  bsearches): a K-side Err mutates no entries and records no deltas — the
  orientation heuristic may still have flipped the block (names unaffected,
  invisible through the direction-aware API); a dup-V Err rolls the placed
  k back — the slides it caused are real, so key-side deltas may be
  PENDING on Err (apply + clear like an Ok; a spot-as-hole placement
  records a no-op event, a push_back records nothing).

consumer:
- `treeblock/` — ALL the tree machinery: `blocks` (Block + BlockTrait/
  BlockOps + the three modes), `walker` (Node contract + walker layers +
  open surface), `treeblock` (TreeBlock marker + constructors), `metadata`
  (the fixup protocol + tree-state types; Pos lives in index now), `store`
  (slot backends + slide/gather/grow primitives), `translator` (the 4-knob
  translator, fn-ptr-specialized over zero/nonzero params), unwired
  `block_cursor`/`leafblock`/`inline_leafblock`, and the tests (live
  `tests/walker.rs`, unwired `tests/store.rs`/`tests/block.rs`).
- `benches/` — `sorted_find_slot.rs` (mask find vs store scan),
  `sorted_insert.rs` (append/prepend/random/runs × insert/lookup/seqread
  vs sorted-vec + BTreeSet, 32-byte items, size via CLI arg — default
  10k, u32 addrs — with a 3s wall budget per structure×workload cell and
  partial-rep reporting; findings + phase breakdown in
  `notes/session_history.md`), `blockmap_insert.rs` (same harness/workloads,
  BlockMap vs BTreeMap + the BlockSet same-run reference — u64 values),
  `bimap.rs` (same harness, BlockBiMap vs a two-BTreeMap bidirectional
  control — insert/lookup-both-sides/remove/iter_pairs; findings in
  session_history 2026-10-15).

notes: `devlog.md` (design log), `session_history.md` (per-session record),
`subtle_bugs.md` (solved nuances + their rules).

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

blockset + blockmap are inline `#[cfg(test)]`: mask (exhaustive scans vs naive
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
gather banking by distance + the LEFT-side multi-segment mirror with
exact hop-pair assertions; front-insert cascades; pending-deltas panic;
dup rejection; remove/reinsert; NoneSlide expansion). blockbimap inline:
holder mirror on BOTH name columns, mixed-direction streams, the dup-v
rollback with pending deltas, 600-op torture, name-space exhaustion.

Run targeted: `cargo test -p doa --lib blockset` / `blockmap` / `mask` /
`treeblock::walker`; uninit/leaks: `cargo +nightly miri test -p
doa --lib <filter>` (slow — recompiles + interprets; blockset, blockmap
and blockbimap are miri-CLEAN; miri runs are expensive — filter to small
subsets, the torture tests dominate).
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Future Work

- blockset: `try_insert`/`try_alloc` split + arena-level occupancy
  decisions; threshold tuning (FREE_STRIDE + the len/8 reversal trigger
  remain unbenchmarked; the turn line is now `TURN_NUM/DEN` and IS
  benchmarked — 3/4 measured 2026-10-15 and REJECTED: −18% random @10k,
  2.25× worse @1M + 2× random-build memory, see session_history);
  holder-applying bench variant
  — the cascade savings (turns/flips are zero-churn) aren't measured by a
  deltas-clearing bench.
- blockset: the 7/8 preemptive turn fires before checking whether the
  spot is already open — delete-max + reinsert-max pays a full turn for a
  free slot (report-only finding; a nearest-hole-within-stride guard is
  the candidate, unbenched).
- blockset: offset turns (window moves — mask + data halves handle any
  offset now; the move op remains, and it must respect `offset < cap` or
  generalize the reversed band — subtle_bugs §20),
  mixed-side/multi-event deltas (`Delta::Pairs` awaits bulk),
  `from_vec`, convenience getters.
- blockset: find-bench density sweep (90–97%) — the bench now runs the
  store side against treeblock's store (the blockset copy is gone).
- blockmap: `value`/`value_mut`/`remove_key` go through `position()`
  without find's beyond-the-extremes fast path — an out-of-range probe on
  the map tier's primary read pays a full bsearch (report-only review
  finding; two-compare guard the candidate, unbenched).
- blockbimap: `get_by_key`/`get_by_value` double-get (find→addr, then
  `get` re-translates + re-reads, then the partner chase) — measured 229ns
  over the keys map's `value` at 1M (decomposition, session_history
  2026-10-15); find-returns-pos + read-once should reclaim ~130ns
  (report-only, unbenched). the chase's insert-side tax (~24% on random
  @1M) is proportional to moved elements — inherent to the bijection's
  partner fixups, not obviously reducible.
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
- known latent flaws (all unexercised): reparent_run's `set_position` jumps,
  the unary-internal-riser panic, packed-ChildPos nav breaking on
  sparse-addressed nodes, the STORES_PARENTS walker hazard class +
  `set_parent`'s Some-return, children()-sortedness unchecked.
- not designed/wired: deletion rebalancing/merges, serialization,
  `leafblock`/`inline_leafblock`, `block.rs` test adaptation.
- trie integration.
