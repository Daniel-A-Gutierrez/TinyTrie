# doa — Dense Ordered Arenas

The problem, stated plainly

A malloc-per-node tree is hostile to the three things a database index wants. Every node is a separate heap object, so traversal is a tour of scattered cache lines. Serialization means walking the whole structure and rebuilding the pointer graph on the way back — pointers into the heap are meaningless on disk. And every pointer is a full usize, eight bytes of address to name something that sits in a collection you could name in two.

The obvious fix — store the whole tree in a plain array — fixes all three: traversal becomes a linear scan of adjacent memory, serialization becomes writing the array out, and pointers shrink to 16 or 32 bits. But it buys you a new problem, and this problem is the entire crate: arrays are terrible at inserting in the middle. Inserting between elements 41 and 42 means either everyone after the gap moves up one — and now every pointer in the tree that names anything past the gap is stale — or you leave the array sparse, and "sparse" is a slippery slope back to a heap.

So the real question DOA answers is: how do I insert into the middle of an array without breaking any pointer, while keeping the array's order meaningful and its density high? Everything in the crate is a consequence of taking that question seriously.

The two conceptual moves

Move one: separate a thing's name from its shelf position. Think of it like a database: the vaddr is a logical row pointer — assigned once when a node is born, never changed — and the physical slot is where those bytes actually sit. The two are linked by the translator, which is nothing more than a tiny, invertible arithmetic formula (four numbers: two offsets, a shift, a rotation) mapping names to slots and back. The point of this separation is that shelf positions can change en masse while names stay perfectly still. When the store needs to double its capacity, it interleaves a fresh empty slot after every element — a "spread" — and every element's slot index doubles. That would normally invalidate every pointer in the tree. Instead, the translator's shift knob drops by one, and every existing name now maps to its element's new slot. Nobody was told anything. Nothing was repointed. The pointers were stable by construction, because the mapping itself absorbed the change.

This is the move that makes serialization credible: names survive relocation, so a name is a durable thing you can write to disk.

There's a subtlety here that is genuinely hard to internalize, and it's worth saying slowly: the numeric order of vaddrs means nothing. They can wrap around the top of the integer range. They're names, not positions. The only thing that carries real order is the physical array — slot 0 always holds the smallest element, the last slot the largest. Every invariant in the crate is phrased over physical order, and every translation trick is judged by exactly one criterion: did it preserve physical order? Once you've absorbed that flip — "the array is the truth, the numbers are just labels" — most of the crate reads differently than it did before.

Move two: mutation reports corrections; everyone else applies them. When a slide shifts a run of five elements up by one, every parent pointer into those five elements is now wrong. The conventional design has the mutator hunt down and fix the pointers itself. DOA can't do that — it has no idea what your tree looks like inside, and that's deliberate. Instead, the mutator finishes its physical work and hands back a small, closed-form description of what it did: "these slots moved by this delta," or "this range doubled its indices." Anything in the world that holds addresses — the block's own metadata, a walker's saved position, the consumer's stack of ancestors, the consumer's node fields — receives that fixup and corrects every address it holds.

This is an inversion of control, and it's the crate's most important structural decision. It's the reason arbitrary consumer trees can plug into crate-owned mutation logic: the crate owns the choreography (what moves, in what order, with what corrections reported at what moment), and the consumer only ever implements one method — "given a correction, fix your stuff." The complexity doesn't disappear; it gets centralized into a handful of functions where it can be reasoned about once.

The three ways to make room

Every space problem in the crate reduces to three operations, in escalating order of disruption:

- Slide — the local fix. There's a None hole four slots away; rotate the intervening run toward it, and the hole lands next to where you want to insert. Cheap, touches only the run between you and the hole. Costs one fixup: the run moved.
- Spread/grow — the global fix. No hole nearby, so double the store and interleave a None between every pair of elements. Now every gap is an insert point. Touches the whole store, but costs no fixups to the tree's pointers — the translator absorbs it. This asymmetry (slides are local but require corrections; spreads are global but require none) drives a lot of the design's shape.
- Cleave — giving up on this block. Out of capacity, out of shift budget: cut the block in two. Done naively this invalidates every cross-block name; done as a rotation, the right half's elements land on interleaved slots with fresh holes between them, and the new block's translator just gets one different rotation parameter. The names survive even the death of the block they were born in.

Why trees, and why the walker

Here's the constraint that shapes the whole architecture: a slide's fixup has to reach its recipients. Somebody must know who points into the moved run. For an arbitrary bag of nodes, that's unanswerable without scanning the world — which is why the crate is trees-only. In a tree, the referers of any moved run are discoverable by traversal: they're parents, and parents are found by walking. But "walking" requires state — where am I, how did I get here — and that state itself holds addresses, which means the walker is also a fixup recipient. That's the third layer of the design: the walker isn't a convenience API over the block, it's the piece of machinery that makes middle-insert possible at all, and its state has to survive the very mutations it triggers.

The three-layer walker split follows from this. The consumer owns the innermost layer (what does your node look like, how do I read its children, set its pointers) because that's the one thing the crate cannot know. The crate owns the middle layer (what does "next" mean in preorder vs inorder vs postorder) and the outer layer (the actual insert/split choreography) because those are where every consumer would otherwise make the same subtle mistakes. The orderings themselves are deeper than traversal conventions: the ordering is the layout. Preorder says a parent sits immediately before its children; inorder says it sits at the boundary between its left and right halves; postorder says it sits after. That placement determines what a split must move, which determines the entire arm of choreography per ordering. Choosing an ordering is choosing your mutation costs.

The hardest concepts, ranked

I'd split them into three kinds — worldview shifts, choreography, and sharp edges — because they fail differently: the worldview ones make everything else unreadable until they click, the choreography ones are just genuinely intricate, and the sharp edges will hurt you if you touch them without respect.

1. Vaddr-as-name, phys-as-truth (worldview). The single biggest conceptual hurdle. The instinct is to treat addresses as positions — everything in computing trains you that way. Here they're opaque, possibly wrapping labels, and only the array's physical order is real. Until this clicks, the translator looks like a pointless layer of indirection and every fixup looks optional. After it clicks, the translator looks like the whole point.

2. The fixup protocol and its ordering discipline (choreography + worldview). Not the mechanism — "apply this remap to your addresses" is easy — but the sequencing rules and why they're load-bearing: corrections must be applied to the walker's own state before it's used to walk; a two-slot reservation must have both slides computed before either moves, because you cannot walk a tree that is half-mutated to find the second one; the run of moved elements must have its parents corrected by a walk that happens before the physical slide it describes. These rules are not stylistic. Each one exists because the alternative is walking a tree in a transiently invalid state — reading a slot mid-relocation, or trusting an index that no longer means what it did.

3. The split driver, especially in-order and postorder root splits (choreography). A root split must create a new root and a new sibling and possibly relocate the old root, each into freshly opened slots, each written before the next is opened, with the walker's own position surviving all of it. The preorder arm is the simple one; the postorder arm relocates the old root via a swap first so the drain has somewhere to land; the in-order arm has the old root keep its slot because the one gap that survives a split un-moved is exactly the one the root occupies. This is the densest code in the crate, and it's dense because it's where all three conceptual moves — naming, fixups, ordering-as-layout — happen simultaneously.

4. The in-order boundary and the parent hop (the subtlest single fact). In inorder, a parent sits at a gap index fixed by DEGREE — not by how many children it currently has. That's counterintuitive (why doesn't the boundary move as children arrive?) and it's fixed that way for one reason: so a split never moves the node being split. Follow the consequence: inserting a child left of the boundary shifts which gap is "the parent's" gap — the parent's identity has to hop over a subtree. One insertion can move a node by a whole subtree's width, and the rule for when that happens (child_idx < DEGREE/2) is exactly the rule for which half a split takes. This is the crate's best illustration that ordering semantics and allocation mechanics are one subject, not two.

5. The reservation model (sharp edge). Slots are Option<MaybeUninit<T>>, and the store can hand out a write-place into a slot that doesn't contain a valid T yet. The contract — a slot may be read only after its reservation's write completes — is enforced by borrowing, not by runtime checks, and the walk-==-slot-order canary is the tripwire that catches a violation before it becomes assume_init UB. Dropping a store with a pending reservation is straight-up UB. This is the one place the crate trusts the caller in a way the type system only half-remembers, and it deserves the respect it gets in subtle_bugs.md.

6. The modes as workload bets (breadth, not depth). Uniform, Anchored, Pluripotent aren't three algorithms so much as three answers to "where will space be needed next?" — answered with different initial translator knobs, different store backends, and different find-space ladders. None is individually hard, but their interaction surface (which one pins the root implicitly, which one grows at the edges and compensates the translator instead of moving anything) is a lot of context to hold at once.


## Workflow

- sessions start from a clean tree: the previous session ends with a commit.
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

# Style
- doc comments are the single source of truth for item outlines - keep them minimal, don't make them a summary, just purpose, invariants, and panics. 
- single line comments may be introduced in long functions to concisely explain what a block of code does
- a comment must never be longer than the source it applies to. 
- function names and variables should be concise but explanatory - avoid arbitrary letters and abbreviations. 
- each file's `//!` header carries its purpose + invariants. this file keeps the conceptual
  map only — item inventories live in doc/ and are generated, never hand-edited.

## Files (lowest level → highest)

Item inventories live in `doc/<name>.md` — generated skeletons (fenced rust,
`///L####` tags jump to source). This section is the conceptual map only; the
files' `//!` headers restate purpose + invariants next to the code.

- `lib.rs` — module wiring + the ordering vocabulary (`RootPos`/`Order`/`Ordering`).
- `index.rs` — numeric trait ladder + type-level const facts underpinning all
  address math; upholds only the numeric contract.
- `translator.rs` — `v2p`/`p2v` translation, fn-ptr-specialized over zero/nonzero
  params; the one hard rule is physical order (phys 0 = min, phys len−1 = max).
- `metadata.rs` — the fixup protocol (`Fixup`/`Fixable`) + walker/block data types
  (`Pos`, `PosAncestry`, `Root`…); `HasRoot` exposes a movable root **phys**.
- `store.rs` — unbounded `Option<MaybeUninit<T>>` slot backends + slide/find/
  grow/spread/split/reservation primitives; the alloc-write-read contract.
- `blocks.rs` — `Block` (store + translator + block data + mode) + the shared
  `BlockTrait`/per-mode `BlockOps` surfaces + the three modes.
- `walker.rs` — `Node`/`SplittableNode` contract + the three walker layers +
  the split driver; `B` is a trait param at every level, `O` is always `B::O`.
- `treeblock.rs` — `TreeBlock` (param-less tree-block marker) + the `walker`/
  `search` free-fn constructors over consumer `From` impls.
- `subtle_bugs.md` — nuanced correctness issues solved, with diagrams; the rules
  they left behind.
- unwired — `block_cursor.rs` (not even declared in lib.rs) + `leafblock.rs` /
  `inline_leafblock.rs` (compiled, dead) + `src/archive/` + `examples/old_btree/`
  (the live consumer is `examples/btree.rs`).

## Testing

`src/tests/` — `store.rs` (fully commented; `store.rs` still wires its `#[cfg(test)]`
module) and `block.rs` (live but unwired code against the deleted pre-refactor API —
needs adaptation, not uncommenting). No btree test file exists. When the block op
surface stabilizes, adapt the store/block invariant tests to the current API.
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Status

Compiles (lib + workspace); `examples/btree.rs` — a B+ tree consumer — runs green:
100 keys through the split driver (leaf/internal splits, multiple root promotions,
height ≥ 2), all anchor kinds, in-run + out-of-run slides.

Untested (compile-verified only, no consumer): the in-order parent hop (split and
insert triggers), the postorder split arms, the `STORES_PARENTS` reparent machinery
(`swap_current`/`reparent_run`/`adopt_node`), `find_2_slots`/`open_two`/`TwoSlide`.
The example is preorder + parent-free; an in/post consumer example, ideally
parent-storing, is the natural next test and would exercise all of them at once.

Not designed/wired: the walker-driven union-node `drop_tree` (design deferred);
`BlockExhausted` cleave handling (arena); deletion rebalancing/merges;
serialization; the test suites; `leafblock`/`inline_leafblock`.

## Future Work

- split hardening — in/post consumer examples (parent-storing) exercising the hop, the postorder arms, and the reparent matrix; a canary negative test (alloc-in-run-without-wire → expect the panic).
- extend the btree consumer (deletes with leaf removal via `remove_child`, underfull merges) and adapt the archived tests.
- `keys()` iter hook on the cursor so B+ shapes share the child-min fetch / separator re-derivation / equal-right routing in-crate.
- ordered iteration over K/V (`IntoIterator` on the walker + `is_leaf` filter) + a range surface.
- arena tier — infallible insert (absorb exhaustion via spread/cleave/readdress), adaptive runtime strategy switching, overprovisioning, subtrees & forwarding (block_id roots), ordering across splits.
- graduation — pluripotent → concrete strategy at len == half_ptr.
- `Fixup::applies` optimization (elide unnecessary runtime checks).
- trie integration.

# Updating the Claude.md
Keep the structure a breadth first tree of the subsections - at the top the 'root' tells a reader what this crate is and what its purpose is. There should then be a 1 line description of each subsection that will follow.
Each per-file section is conceptual only: what the file is, its broad purpose, the invariants it maintains - a few lines at most. Item inventories are NOT maintained here: they live in `doc/<name>.md`, generated by `skeletonize.py` from the source doc comments. To change an outline, change the source comments and rerun `skeletonize.py`.
The document should be ordered so as to maintain a contextual foothold for the reader - the root covered big picture, so ordering the subsections to build up from the lowest level -> highest level makes sense logically.
Maintain this section at the end of the claude.md .