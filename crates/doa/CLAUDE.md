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
- **no walker, no `Fixable`, no anchor discipline.** `gather_slots` is
  the WORD-CLASSIFIED ladder (2026-10-20): DIRECT (spot a hole) / NEAR (a
  hole in k's own word — one plain slide, or a word-bounded bank of
  `1+streak` holes when the locality streak is hot and the hole is past
  FREE_STRIDE) / BORROW (a neighbor word under the HIGH line banks
  `BORROW_MIN+streak` holes — the lendable surplus; a milked word lands at
  High and stops lending) / the FORK — cold streak (< STREAK_BANK)
  balances, hot banks; both arms end in `rebalance_region`, and a failed
  bank (hole past len/2) falls to the same growth-rebalance (its tail
  arm). `rebalance_region` scans words outward (sparser side first) until
  density < FREE_NUM/FREE_DEN strictly (= the stride-FREE_STRIDE lattice
  capacity — the stop IS the stride cap), grows the tail via
  `push_slots` when the scan hits the end, and past the name ceiling
  (turn-refused, §18) slides from the nearest hole. open()'s APPEND
  FASTPATH sits above the ladder: a solid tail run within FREE_STRIDE of
  the edge extends the block and slides ≤ FREE_STRIDE elements. the
  ladder is INFALLIBLE — no bails, no forced retries, no Option plumbing.
  No run-walk fixup
  choreography, no ancestry.
- **Deltas = one compact event.** `enum Delta { NoneSlide{from,to},
  Gather, Rebalance{base,at,stride,mask}, Pairs }` — the common
  contiguous slide records the hole's `from → to` (elements between
  shift one step toward `from`; `from == to` = no-op direct place); a
  bank records `Gather(Vec<NoneSlide>)` — the slide chain, nearest hole
  first, each slide in post-prior-slides coordinates (identical to its
  pre-read position: a hole's slot is never touched before its own
  slide; a two-sided gather is two chains' worth, side told by
  `from > to`). a `Rebalance` is the region's water-leveling rewrite:
  the live+1 elements (k at rank `at`) redistribute onto the lattice
  `u(i) = i + i/(stride−1)`; the stored mask is the PRE-move occupancy,
  and `Deltas::iter` re-derives every pair from it in two passes
  (right-movers descending, left-movers ascending — first-match safe by
  strict monotonicity of targets in rank), while `slides()` reports the
  region as one synthetic link (skipset unions it). k rides the lattice
  so the rebalance IS the open — one insert, one event; a second record
  would overwrite the first (that constraint shaped the design).
  `Deltas::iter` also expands plain slides to 1-step cascades
  (hole-adjacent first), gathers to ONE pair per moved element carrying
  the FULL hop (crossing count, `w − segment index` — walked
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
  §21), `copy_region`/`count_below` (region occupancy → a fresh Mask,
  and prefix popcount — `rebalance_region`'s from-record and boundary
  rank; naive-swept at every word edge), `rotate_mask` (the turn's mask half: interleave + one band
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
  untouched (it speaks names; names didn't change). trigger: mean insert
  position (`pos_sum/len`, doubling at turns, reflecting at flips) below
  len/8 past a 4·stride warmup, Z-GATED on the variance (`pos_sq_sum`,
  reflected `len³−2·len·S+Q` at flips, quadrupled at turns): the low mean
  must clear 2σ of the observed spread — alternating streams (front-landing
  chunks from either direction) have huge variance and never look
  front-biased, so the trigger can't thrash; a tight front cluster
  (variance ≈ 0) flips as before. multiply-only hot path: the mean gate
  runs first, the variance test only when it already says front-biased.
  SkipSet drives reversal MANUALLY (`should_reverse` + `reverse`) — its
  insert path never auto-flips; plain BlockSet inserts keep the auto
  trigger. see subtle_bugs §20 for the reversal turn's band + the
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
- `mask.rs` — dense occupancy bitmap (see the blockset section) + the
  bidirectional cached-word steppers (`step_up`/`step_down`/`word_at`)
  + `MaskCursor` — the slot-walk state machine (slot/ghost/dir/word
  cache) the set/map cursors wrap; walk-path methods `#[inline(always)]`
  end to end (subtle_bugs §23).
- `translator.rs` — the set/map tier's 2-knob translator, fn-ptr per
  direction + the reversal/turn maps; the one hard rule is position order
  (pos 0 = min, pos len−1 = max).
- `serialize.rs` — the `Archive` trait: a stored K decomposes into a
  fixed-size POD `Handle` (slots hold handles; handles ARE the
  serialized form — heap-relative offsets, LE-canonical, so the
  (slots, heap) pair serializes as-is) + bytes in an append-only side
  heap. `cmp_key` (stored handle vs query key — the set's total order),
  `view` (GAT borrow form), `materialize` (spent handle → owned value);
  `Handle: Copy + Eq` (Eq structural — skipset borrow holes are detected
  as repeated handles) + `const MIN: Handle` (the skipset borrow seed at
  tier-region heads; compares ≤ every real handle).
  impls: integers/char inline by macro (Handle = Self), String (8-byte
  BE-loaded zero-padded prefix for inline compares + ≤8-byte inline
  cutoff — no heap bytes), Vec<u32> (LE, 4-aligned). heap offsets are
  internal, never held outside the block — a future vacuum compacts
  without fixups. removal leaks heap bytes (v1).

tiers:
- `blockset.rs` — the set tier: `BlockSet<K: Archive, A>` (slots hold
  `K::Handle` copies over an append-only side heap; drop machinery gone —
  handles are POD, heap bytes leak on remove by design) + `Deltas` + the
  fetch-gather ladder + the z-gated reversal trigger + the differential
  `Iter` over SetBits/SetBitsRev; `find`/`contains` fast-path beyond
  either extreme (two compares, no bsearch — sequential probes land past
  the tail). the `insert_at`/`push_beyond` pair drives SkipSet's descent:
  insert_at is UNSAFE, window-bounded (`from ≤ to ≤ cap`, bounded
  `bsearch_bounds` inside), NEVER auto-reverses — SkipSet owns the flip
  (`should_reverse`/`reverse`); push_beyond takes a caller-guaranteed
  beyond-tail key. tier/diagnostic accessors: `slot`/`first_live_pos`/
  `cmp_key`/`pos_of` (pos_of unchecked — live addrs only) + `slides()`
  (the raw NoneSlide chain — SkipSet coalesces intervals from it, cheaper
  than pair expansion). the `Cursor` family: `seek`/`seek_after` →
  `next`/`prev`/`key`/`addr` over `MaskCursor`, invalidated by any
  mutation. the pos `epoch` (bumped by turn/flip — the skipset staleness
  guard), `pos_sq_sum` (the z-gate's variance half), the always-on turn
  census statics, and `live_run_lengths` are SkipSet/bench-facing.
  `Deltas`' event recording, `NoneSlide`, and `FREE_STRIDE`/`L2_BYTES`
  are `pub(crate)` — blockmap shares them.
- `skipset.rs` — the skip-list index over BlockSet: `SkipSet` +
  `STRIDE`/`STRIDE_SHIFT`/`STRIDE_MASK` consts (power-of-2 — slot math is
  shift/mask; runtime divisors never compile to shifts). tier0
  WINDOW-FIRST sampling (slot j = the first live item in base window
  [j·STRIDE, (j+1)·STRIDE) via `next_set`); upper tiers lift the block
  MIN — the first REAL value of each STRIDE-wide below-grid run
  (`lifted`: head real iff it differs from its left neighbor — O(1) fast
  path, forward scan only on holed heads). NO MASK (2026-10-21, the
  borrow scheme): empty slots BORROW their left neighbor's handle
  (`K::MIN` at the region head) — real values strictly increase, so a
  repeated value IS a hole, and the descent skips repeats exactly as the
  old mask skipped clear bits ("doesn't narrow, only widens" — the safe
  direction). Store is plain `Vec<K::Handle>` (`vec![K::MIN; total]` at
  rebuild — no MaybeUninit; every in-width slot initialized, reservation
  slots never read), ONE allocation, regions [top…tier0] front-to-back,
  reservations sized 2× the width chain at 2·cap (covers one turn and
  the cap-creep ceiling). STALENESS IS THE SCHEME'S SHARP EDGE
  (subtle_bugs §26) — two sites are correctness-load-bearing, one is
  tightness: the descent HEAD must seed its hole-check (prev from slot
  lo−1 — an inherited lo can name a borrowed grid origin — and K::MIN
  at lo==0, where a Desc head borrow once compared Greater and pinned
  bhi to 0, every probe dead); maintenance writes must go through
  `set_cascade` (rewrite the run of holes to the right that copied the
  old value — a hole's content depends on its left neighbor's VALUE,
  which apply_slide's position intervals don't cover); the tail-push
  empty→real at a gridline re-lifts the upper gridline chain as
  TIGHTNESS only (a stale borrow still equals its left neighbor and
  repeat-detects; a stale lift is widening-only; `tier0 > 0` guard).
  `Store::holes`
  (set by any borrow, reset by rebuild) gates the descent's per-slot
  repeat-check OFF entirely for dense stores — the fast path that pays
  for the scheme (−16% lookup at 1k append vs the mask version; holed
  stores pay ~+3–5%, DRAM-scale wash; upkeep wash). upkeep is
  incremental: slide chains coalesce into ONE union interval
  (idempotent re-reads cascading [ceil(lo/s), hi/s]); tail pushes update
  a window's first iff the placed item IS the first; silent moves
  (epoch mismatch — turn or the manually-triggered flip) re-read every
  window's first; `rebuild()` only past the reservation. the descent
  scans each tier's range for last ≤ k / first > k — the tier0 range
  maps to base positions (`bhi` clamped to cap) and resolves via
  `find_window`/`insert_at` (windowed `bsearch_bounds` — a linear
  walk experiment lost ~6ns/insert at 10k and was reverted 2026-10-21;
  young coarse tiers can hand down windows up to top_width·stride wide,
  so window code must not assume ≤ stride — subtle_bugs §27). STRIDE=16
  settled (2026-10-21 sweep, 1k–100M: lookup floor sits at stride 16
  cache-resident — 32 actively loses to the plain bsearch at 1k, 30 vs
  23 — and is a wash at DRAM scale; upkeep tax grows as stride shrinks,
  4 pays +27–34% insert; the root scan must stay ≤ stride — the s²
  misreading measured +165ns/insert at 1M from the linear root scan
  alone). `SkipSet::insert` NEVER lets the base auto-flip: it triggers
  reversal itself (z-gate), then descends. removal unsupported (v0).
  bench-facing: the A/B phase hooks
  (`insert_maintain_only`/`insert_descent_only`) are `#[doc(hidden)]`.
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
  extremes fast path as blockset's. the `Cursor` mirror (`seek`/
  `seek_after`, `next`/`prev` yield pairs, `entry()` peek, `addr`) —
  blockset's machine (`MaskCursor`, over pairs).
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
  cursors over both native orders: `seek`/`seek_after` → `Cursor` (key
  order, yields `(&K, &V)`) and `seek_by_value`/`seek_after_by_value` →
  `ValueCursor` (`(&V, &K)`) — blockmap cursors with the partner chased
  one hop per yield (like the iters); `addrs() -> (K-side, V-side)` on
  both.

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
  session_history 2026-10-15), `cursor.rs` (the cursor family vs
  `BTree{Set,Map}` `range` — the stable surrogate, std's cursor API is
  nightly-only: fwd = seek + 100 `next`, back = seek_after + 100 `prev`,
  both tiers × 4 workloads, cursor-vs-range orders verified over a probe
  prefix; + scan rows — full-set iter vs cursor fwd/back, ns/elem
  (findings 2026-10-16 (b): parity at both scales; the seek+100 cells
  are bsearch + one cold line, not stepping); the optimization ladder +
  final numbers in session_history 2026-10-16), `strings.rs` (the string
  mirror: BlockSet<String> vs SortedSet vs BTreeSet × 3 key shapes —
  fixed16 / var / digits — the StrHandle cutoff's cells; findings
  2026-10-19), `skipset.rs` (BlockSet vs SkipSet vs BTreeSet ×
  append/random/runs — insert (base / wrapper / maintained) + lookup +
  seqread; a maintained cell builds tiers at the half then times the
  rest; named-arg "phases" adds the A/B paired insert-path ladder
  (upkeep/descent/search deltas — paired medians, chained cells measured
  garbage once); every run prints the turn/slide census (turns timed
  inside grow_rotate — rare, O(cap), so clocks are free — + slide
  moved-elements from the deltas + the desert census via
  `live_run_lengths`; findings 2026-10-19: turns 1–3% of build, the
  gather tail is count-bounded and quadratic in desert depth).

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
dup rejection; remove/reinsert; NoneSlide expansion). cursor batteries
(2026-10-16): blockset/blockmap `assert_walks` — both seeks × both walk
directions at EVERY boundary over a holed stride-built block + a flipped
block, peek/addr checks, ghost round-trips; bimap's macro battery over
both orders (descending keys force the flip, values ascend — mixed
internal directions); mask `steps_match_naive` (single steps + full
cache-riding walks vs naive at every start, the cache invariant asserted
per hop). sized for miri — 60-key sets over a 67-wide key space (every
boundary still swept; the flip still trips, gathers still bank), 70-bit
mask: ~2 min.

Run targeted: `cargo test -p doa --lib blockset` / `blockmap` / `mask` /
`treeblock::walker`; uninit/leaks: `cargo +nightly miri test -p
doa --lib <filter>` (slow — recompiles + interprets; blockset, blockmap
and blockbimap are miri-CLEAN; miri runs are expensive — filter to small
subsets, the torture tests dominate).
⚠ Running the full `cargo test` in this crate has crashed the IDE out of memory in
the past — run targeted tests.

## Future Work

- cursors: `seek`/`seek_after` pay a full bsearch for beyond-the-extremes
  probes — find's two-compare guard is the candidate (report-only review
  finding, unbenched; the cursor bench shows the bsearch dominating at
  DRAM scale, so the guard is a cache-resident-scale win only). bimap's
  `Cursor`/`ValueCursor` chase wrappers are plain `#[inline]` over
  `inline(always)` walks — one potential outlined call per yield,
  unmeasured.
- blockset: `try_insert`/`try_alloc` split + arena-level occupancy
  decisions; threshold tuning (the z-gate's 2σ remains
  unbenchmarked; the turn line `TURN_NUM/DEN`: 3/4 REJECTED twice
  (2026-10-15/19 — the loss is the LAYOUT: doubled cap → sparser
  probes + stretched deserts, random @1M lookup 347→1132), 7/8 the
  settled value until 2026-10-20's 5/6 — the ceiling = the scan stop,
  holding density under workability; 5/6 won random −45% insert/
  −73% movement at 1M but runs' reads sit on THRESHOLD LUCK at
  len 1M (between the 5/6 and 7/8 lines of cap 2^20, one extra
  doubling fires) — a size sweep across the boundary is the missing
  measurement; BORROW_MIN/streak-boost also unbenched);
  holder-applying bench variant
  — the cascade savings (turns/flips are zero-churn) aren't measured by a
  deltas-clearing bench.
- blockset: multi-word banking — the bank arms are word-bounded
  (BORROW ~1 event per ~10 inserts per word), so runs still pays ~+9%
  insert over the fetch baseline at 1M; widening banks re-digs the
  deserts the rebalance arm levels — the fork trades them (see
  session_history 2026-10-20 for both directions measured).
- blockset: the preemptive turn (now the 5/6 line) fires before checking whether the
  spot is already open — delete-max + reinsert-max pays a full turn for a
  free slot (report-only finding; a nearest-hole-within-stride guard is
  the candidate, unbenched).
- blockmap: ladder port — blockmap still runs the old fetch-gather
  distance ladder (its own copy; FREE_STRIDE now 6 there too); a
  Rebalance port would touch its slide/turn plumbing but not bimap
  (its chase consumes blockmap events, which never include Rebalance).
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
- skipset: removal (v0 — a mask clear + the window's first re-read;
  the deltas protocol already reports the slide class). auto-stride +
  an L2 build gate — tiers lose ~35% on lookups at 10k (cache-resident
  base); the stride should derive from L2_BYTES/handle size, likely as
  a const-generic once it un-defers. skipset removal of the epoch's
  manual-flip coupling to blockmap's auto-flip (the two insert paths
  differ under tiers). insert_at spot-picking: a linear window walk
  measured −6ns/insert WORSE than the windowed bsearch at 10k random
  (branchy per-live-slot stepping vs branchless cmov probes) and was
  reverted, but its one good idea stands — after `bsearch_bounds`
  returns the gap, one `next_clear` probe inside [spot, first-greater)
  picks the gap's first HOLE as the insert spot when one exists (free
  space, no slide — unbenched; movement was 3.46/insert at 10k so the
  ceiling is one slide's worth).
- blockmap/bimap: the Archive port — keys AND values get handles over
  one shared heap (blockmap's parallel MaybeUninit vecs + mask become a
  three-way lockstep with the heap; bimap untouched — its values are
  names).
- not designed/wired: deletion rebalancing/merges, serialization
  (the (slots, heap) pair IS the serialized form — validation is the
  only load-time work), `leafblock`/`inline_leafblock`, `block.rs`
  test adaptation.
- trie integration.
