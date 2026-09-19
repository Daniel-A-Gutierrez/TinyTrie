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
  prune stale entries), commit.
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

- `lib.rs` — module wiring + the ordering/side vocabulary (`RootPos`/`Order`/`Ordering`/`Rel`).
- `index.rs` — numeric trait ladder (`Num`/`UnsignedNum`/`Addr`, the ex-`BlockIndex`)
  + type-level const facts underpinning all address math; upholds only the numeric
  contract.
- `translator.rs` — `a2p`/`p2a`/`adist` translation, fn-ptr-specialized over
  zero/nonzero params; the one hard rule is position order (pos 0 = min, pos
  len−1 = max).
- `metadata.rs` — the fixup protocol (`Fixup`/`Fixable`/`CursorState`) + the
  position/child types (`Pos`, `ChildPos`, `PosAncestry`, `Root`…); `HasRoot`
  exposes a movable root **pos**.
- `store.rs` — unbounded `Option<T>` slot backends (slots hold initialized values;
  no reservation model) + slide/find/grow/spread/split primitives.
- `blocks.rs` — `Block` (store + translator + block data + mode) + the shared
  `BlockTrait`/per-mode `BlockOps` surfaces + the three modes.
- `walker.rs` — `Node` contract + the walker layers: `NodeCursor`/
  `NodeWalker`/`NodeWalkerMut` (consumer mask; the walker IS its own state —
  `Fixable` + internal `Snapshot`/save/load/set_position), `TreeWalk` (nav,
  per-ordering: pre + in; post unimplemented), `PreOrderWalk`/`InOrderWalk`
  (the open surface — slot-moving ops consume the walker; rotations keep it),
  and the sealed `TreeWalkHelper` (slide engine + open engines; `Anchor` is
  internal). `B` is a trait param at every level, `O` is always `B::O`;
  traversal assumes packed ChildPos (rank == slot) — sparse-addressed nodes
  need children()-based sibling walks, unimplemented.
- `treeblock.rs` — `TreeBlock` (param-less tree-block marker) + the `walker`/
  `search` free-fn constructors over consumer `From` impls.
- `subtle_bugs.md` — nuanced correctness issues solved, with diagrams; the rules
  they left behind.
- unwired — `block_cursor.rs` + `leafblock.rs` / `inline_leafblock.rs` (mod decls
  commented out in lib.rs — uncompiled, unported to addr/pos) + `src/archive/` +
  `examples/old_btree/` (the live consumer is `examples/btree.rs`).

## Testing

`src/tests/walker.rs` is LIVE on the open surface: a preorder B+ consumer (Vec
nodes, DEGREE 4 — consumer-driven splits) with 240-key torture (ascending/
descending/stride) + hand-assembled anchors + held-addr fixup delivery, and an
in-order DEGREE=2 binary consumer for rotate_left/rotate_right (root / leaf
riser / mid) — validated by structural checks (separator re-derivation from
child mins, leaf order, reachable == occupied) and walk order vs strictly
increasing positions; miri-clean.

`src/tests/store.rs` stays UNWIRED (written against the addr/pos surface;
re-enable after porting) — reference-model torture + exhaustive slide/find/
find-2 matrices for both backends, spread/split/pop coverage, drop accounting,
contract panics. `block.rs` stays unwired (pre-refactor API — needs adaptation).
Run targeted: `cargo test -p doa --lib walker::tests` / `store::tests`;
uninit/leaks: `cargo +nightly miri test -p doa --lib <filter>` (slow —
recompiles + interprets).
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Status

Walker-surface refactor landed (2026-09-19). The crate no longer drives tree
mutations — it supplies OPEN SLOTS at ordering-correct anchors; all wiring
(node inserts, drains, reparenting, root promotion) is consumer-side, against
the block directly. The pieces:

- consumer mask flattened: the walker IS its own state (`NodeCursor` reads +
  consumer-impl'd `descend`; `NodeWalkerMut: Fixable` + `Snapshot`/save/load/
  set_position (internal machinery) + set_child/clear_child/set_parent);
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

Deferred: `find_n_slots`/`open_n` (the swap-minimal N-gather — a worktree
agent attempt died on output limits; nothing landed, `NoneSlide` untouched);
`split_block` body; store tests port (still unwired).

Not designed/wired: sparse-ChildPos nav (children()-based sibling walks —
one-kid in-order nodes are reference-walked in tests, not TreeWalk'd);
deletion rebalancing/merges; serialization; `leafblock`/`inline_leafblock`;
`block.rs` test adaptation.


## Future Work

- gather machinery — `gather_none`/`apply_gather_none` (store tier): swap-minimal
  N-None gather (adjacent Nones crossed by one move), plural plan/fixup type
  beside `DoubleSlide`, then `find_n_slots`/`open_n_*` on top + the 240-key
  reference-model store tests port.
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
