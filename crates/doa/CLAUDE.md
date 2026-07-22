# doa — Dense Ordered Arenas

Consider a btree that mallocs each node. Three costs: 8-byte pointers between
nodes; nodes scattered across RAM (cache-hostile, no prefetch); and
serializing to/from disk is painful — no clean node→offset mapping unless you
page-align, and even then deciding which node goes where in the file is
non-trivial.

doa is an alternative: store an ordered sequence **contiguously in blocks,
addressable by small pointers** (i8+). A subtree is one `block_id` (a `usize`)
containing nodes that reference each other with i8/i16 internal pointers; a node's
value can itself be a `block_id` forwarding to another block/tree. Contiguous
storage — even with `None` gaps between elements — means iteration is a linear
scan the CPU prefetches aggressively, and stays fast at large sizes;
serialization is writing the contiguous bytes.

The crate preserves the **ordering** of the sequence through mutations, not the
*pointers*. Pointers move when a block reorganizes (spread/split on exhaustion);
that change is **reported via `InsertDelta`**, not hidden, so the consumer can
remap. The consumer enforces whatever structure-level invariant it needs
(preorder for a binary tree so leftmost-descent is a sequential scan;
prefix-chasing for a radix trie) on top of the stable ordering. Even with no
tree at all — "node" = key — the stable ordering makes this a sorted array with
~O(log n) insert instead of O(n).

It exposes two tiers:

- **Block** — a fixed-width-addressable run of the sequence. `try_insert_*` /
  `remove` return a `Result` / `InsertDelta` describing what moved or what
  failed; the *consumer* decides how to respond to exhaustion. A block will not
  split or shove items onto other blocks on its own — by design. A consumer may
  use blocks directly, keeping their own `Vec<Block>` as their arena.
- **Arena** — automatic block management. The arena runs adaptive **strategies**
  so blocks optimize for the workload at runtime, dodging n² insertion and
  address exhaustion; `arena.insert_*` is effectively infallible. This is where
  log(n) insert lives — the block layer alone can't promise it.

**Status — work in progress, currently does not compile.** `block.rs` is
mid-refactor of the address model (`addr_range` / `ptr_root`); `strategy.rs`
and tests still call removed functions (`max_magnitude` / `half_ptr` /
`assert_capacity` / `Block::new`). `strategy.rs` is in particular malformed —
the description below is the **intended design**, not always the current code.
Fix the seam before trusting any signature.

## The block

A `Block<T, PTR = i16, const OVERP = false>` is one contiguous run of the
sequence, addressed by `PTR` (i8..isize). Storage is `VecDeque<Option<T>>` — a
slot per addressable position, `None` where a gap sits.

### Address layout

A virtual address maps to a physical slot by `phys = (virt + virt_offset)
>> addr_shift`; inverse `virt = (phys << addr_shift) - virt_offset`. Two knobs:

- **`addr_shift`** spreads physical slots across the signed address space
  (`PTR::MIN..=PTR::MAX`) — stride `1 << addr_shift`. **Spreading does not
  increase capacity** — an i8 addresses 256 positions regardless. It trades
  dense packing for *headroom* at the ends, so appends/prepends have addresses
  to grow into before the block must reorganize.
- **`virt_offset`** slides the window — the mechanism that keeps existing
  addresses stable when the physical buffer shifts (notably `push_front`).
- **Address bounds are the full type range** (`PTR::MIN()..=PTR::MAX()`, e.g.
  i8 = `-128..=127`). The range includes `MIN`, so negatives are *generated* (by
  `push_front` negating a positive offset) and **never mirrored**: never compute
  `-addr` for an address — `i8::MIN` isn't negatable. Derive the low bound from
  `MIN()` directly, not `-MAX()`.

### The sparse AP — aligned positions

The **AP** (*aligned positions*) is the stride-spaced grid of slots `find_slot`
walks when looking for a reusable `None`. A slot is on the AP iff
`(phys + v_off_phys) & none_mask == 1`; its stride is `none_stride = none_mask
+ 1`, independent of the address stride `1 << addr_shift`. `None` gaps are
pre-stocked on the AP so the walk is a short bounded stride-hop, not a scan.
**The gaps are the whole mechanism** separating O(n) `Vec`-insert from cheap
mid-insert: a mid-insert reuses a nearby AP `None` instead of shifting a tail.

### Mutation surface and the `InsertDelta` contract

- `push_back` / `push_front` are address-stable. `push_front` bumps
  `virt_offset` by `1 << addr_shift`, which cancels the physical shift
  `VecDeque::push_front` causes — stable for *all* `addr_shift`, not just 0.
  (Don't "simplify" to `+= 1`; a regression test locks it.)
- `try_insert_before` / `try_insert_after` go through `find_slot` + the
  strategy; `remove` takes a slot and may shift the resulting `None` to a nearby
  aligned position. All return an **`InsertDelta`**:
  - `Free` — placed in a pre-existing `None` (or pushed); address-stable, no remap.
  - `Move` — elements shifted; carries `addr_delta` (`minus << addr_shift`),
    the remap the caller applies to its pointers.
  - `BlockSplit` — placeholder (see Arena).
- The consumer reads the delta to fix up its pointers. **The block does not
  shift-to-create a gap on insert, and does not split** — those are the
  consumer's / arena's job.

### Returns a `Result` by design

`try_insert_*` / `push_*` can fail with `NotFound::OutOfBudget` (no gap within
stride-budget) or `AddressExhaustion` (next address not representable in `PTR`).
A bare block surfaces the failure and lets the caller decide — it will not
overstep into a split or a shove. A consumer using blocks directly is the one
who knows the structure's semantics, so the decision is theirs.

### Strategies — why they exist

A block's address layout is a **bet on its workload**. The four strategies are
four bets; each optimizes for one insertion pattern and loses on another. The
arena (at the arena tier) picks the bet per block and changes it when proven
wrong.

**Random — the intended case.** A random-optimized block spreads its elements
across the *whole* address range so `None` gaps sit throughout. A mid-block
insert then reuses the nearest gap with a short stride-walk (`find_slot`,
budget-bounded) — O(budget), not O(n). This is the win: insert anywhere without
shifting a tail.

> *State:* i8 block, cap 8, len 4, stride 32 (`addr_shift 5`, `v_off = 128`).
> Spread from a dense 4-element block left the elements at the **even** phys
> slots — `Some` at `[-128, -64, 0, 64]` (phys 0,2,4,6; phys 0 stays put since
> `2*0 = 0`) — and opened the **odd** phys slots as AP gaps — `None` at
> `[-96, -32, 32, 96]` (phys 1,3,5,7). Insert near `0` (phys 4): `find_slot`
> walks the AP outward, finds the `None` at `-32` (phys 3) or `32` (phys 5)
> within budget → places there. Cheap; no tail shifted.

**The flaw that motivates *append*.** The spread that makes mid-inserts cheap
spends the address range on gaps throughout, leaving **less than a stride of
headroom at the back**. So an append-heavy workload on a random block hits the
wall almost immediately:

> *State:* i8 block, random, cap 4, len 4, stride 64, addresses `[-128, -64, 0,
> 64]` (spread across the range; top at 64). *Operation:* `push_back`. Full →
> grow + spread (stride 64→32); existing addresses preserved, one new back slot
> appears at `96` (64 + 32). **One** usable append. The next `push_back` would
> need `128` — not representable in i8 (max `127`) — so the block must grow +
> spread *again* (stride 32→16), an O(n) redistribution, to free one more end-slot
> (`112`). Each subsequent append hits the same wall and pays the same O(n)
> reorg: **O(n²) for a push_back sequence.** The layout was shaped for random
> inserts, but the workload was appends → mismatch → quadratic.

*Append's bet:* `addr_shift = 0` — elements dense at consecutive low addresses
`[0,1,2,3]`, the **entire** upper range free. Append just takes the next address
up to `PTR::MAX`; no spread, no shift, no wall. *Blind spot:* a mid-block insert
has no pre-stocked gap nearby (small budget) → `OutOfBudget` fast → expensive
mid-insert.

**Prepend** is the mirror — dense at the top, the lower range free for sustained
`push_front`. Same mid-insert blind spot as append.

**Pluripotent** is "I don't know the workload yet." A conservatively *small*
block (`cap ≤ half_ptr`) kept under the address ceiling so it can serve *either*
appends or mid-inserts for a while without reorganizing — and once the pattern is
clear it **graduates** into the matching concrete strategy. Its reason for
existing: committing early to the wrong concrete strategy costs you (the
quadratic above, or the append-block's mid-insert penalty), so it buys time to
find out.

## The arena

`Arena<T, U, I>` owns `VecDeque<Block>` (blocks linked `prev` / `next` for
ordered iteration) plus a small queue of recent insert hints. It exists to take
the decisions a bare block refuses.

- **Infallible insert.** `arena.insert_before` / `insert_after` are
  effectively infallible: when a block would return `NotFound` or
  `AddressExhaustion`, the arena responds — spread, split, or readdress — so the
  caller never has to. (Skeleton today; `insert_*` are `todo!()`.)
- **Adaptive strategies at runtime.** The arena assigns each block a strategy
  at birth and *changes it* when the workload proves the bet wrong — a random
  block getting hammered with appends is reshaped before the quadratic bites.
  This is how the arena targets log(n) insert, which the block layer alone
  cannot promise.
- **Overprovisioning.** `OVERP = true` widens `PTR` beyond the address space the
  block needs (e.g. i32 ptrs for an i8-scale block), making log(n) insert *easy*
  — but it can double the size of pointer-heavy structures (every internal
  pointer widens). A memory/insert-speed trade the arena-level consumer makes.
- **Subtrees and forwarding.** A subtree stores one `block_id: usize` and uses
  small `PTR`s *within* the block for node-to-node references; a node's *value*
  can itself be a `block_id` forwarding to another block/tree. The small-pointer
  payoff: an 8-byte malloc pointer becomes an i8 within a subtree, with one
  `usize` at the root.
- **Ordering across splits.** A split partitions the ordered run into adjacent
  blocks linked `prev` / `next`; iteration is a contiguous *logical* scan across
  the linked list even though storage is now several physical buffers. The
  linked list is the ordering-stability mechanism across splits, not just an
  iteration convenience.

## Not built / historical

**TODO:** spread / split (block-level primitives and the arena's auto-split),
graduation (`pluripotent` → concrete at `len == half_ptr`; `post_insert_check` is
a no-op stub), prepend gap-insertion, Block `iter` / `iter_mut`, trie integration.

**Historical (do not revive):** `circular_array.rs` is gone (`Block.buf` =
`VecDeque<Option<T>>`). The `MAX` const generic is gone (replaced by `OVERP:
bool`). `BlockIndex::sqrt_max` and the `BlockIndex` trait name are gone (→
`SignedBlockIndex`). Any claim spread/split or the arena are "done" describes a
prior, now-divergent session.


# Table Of Contents of doa.md
Structure (L1) — file orientation: newest top-level entries first.

  Bit rotation (L4)
    - split an in-order binary tree into two slices without invalidating ptrs =>
      lay out non-contiguously with gaps, rotate ptrs left 1 (doubling) to remap;
      left half just +1 to shift, right half subtracts midpoint then rotates.
    - left/right remap math => left: new=(old+1)<<1, v_offset=1 (wrapping);
      right: new=(old-(M+1)).rotate_left(1), v_offset=MAX-1.
    - is the offset needed? => midpoint sits at MIDPOINT.rotate_right(rot)→phys
      MIDPOINT; offset swaps which of 1/128 is free — "fine i guess." [open-ish]
    - translation fns => virt_to_phys=(v+offset).rotate_left(rot);
      phys_to_virt=p.rotate_right(rot)+offset. (f(f(v,1,1))≠f(v,2,2) flagged.)
    novel: bit-rotation (not shift) as the split remap primitive; odds-gapped
      layout keeping midpoint free for the new root; split_and_rotate returning
      [left,right] each with a new root from the old root's children.

  Continued. (L115)
    - first iteration caps parent tree max, but must store leafnode len+cap =>
      store len+cap in a ptr variant; parent = union over u16+len+cap. "cleanest."
    novel: union-over-ptr packing leafnode header (len+cap) with no discriminant.

  Roadmap cotd. (L124)
    - what to build first / how to abstract => minimal BTree-forest thing first,
      then abstract reusable bits (Forest, BTreeMap, SortedVec, LinkedList,
      NibbleTrie). Pattern: build structure → refine reusable bits.
    · Questions rn (L136)
      - how does partitioned work? => only paired with pluripotent; pluripotent
        uses half an address shifted so top or bottom half is empty (append→top,
        random→bottom).
      - leaf blocks are variable/sparse/ordered, don't move on split => inherit
        mode from parent; random leafnode ptr shifts to stay stable on arena
        double.
      - adaptive partition size? => formalize µ-blocks (uBlocks): tiny sparse
        arrays, len+cap as u8s, grow by borrowing capacity from neighbors +
        updating parent.
      - no room for a new header when splitting a full block => per-strategy
        side data: Vec<Option<nonzero u8>>, u8 generic on S.
      - inline len+cap vs direct Inode→LNode association => inline len+cap
      side data: Vec<Option<nonzero u8>>, u8 generic on S.
    - inline len+cap vs direct Inode→LNode association => inline len+cap
      (saves space for big leaves); header = union of PartitionHeader/Leafnode,
      no discriminant, no branch.
    novel: uBlock growing by neighbor-borrowing; per-strategy side-array of
      nonzero u8s; PartitionHeader/Leafnode union; "partitioned only with
      pluripotent."
  · Leafblock (L191)
    - discrete type or inside block? => separate for now (interface too
      different from block's).
    - where does the header live? => options: inline union/enum, side-vec, or
      union over the parent inode's internal ptr (UPtr{internal:u32,
      leaf:(ptr:u16,len:u8,cap:u8)}). Don't presume inline is the only strat.
    novel: leaf header encoded into the parent's ptr-union variant.
  · Addressing (L202)
    - more address space: extend parent ptr+link physical, or rotate via
      parent's shift? => explore rotation; pluripotent constraint
      log2(cap)+shift ≥ bit_width/2.
    - no free address space without an artificial MAX => set max len (e.g.
      2<<16) to free 16 bits; virt>>shift=phys, <<log2(M) guarantees spaces;
      not every phys slot addressable (one per FANOUT/2).
    - fine-grained biasing above halfptr::MAX risks n² off-strategy =>
      pluripotent to cap, then 'refill' addr_shift or send to 0 + readdress.
    - represent multiple slices as one chunk => the space between 2 parent
      ptrs; internal rep [T;MIN], frees log2(MIN) bits for NodeLen in MINs.
    novel: artificial MAX reclaiming address bits; sparse-addressable slots
      (one per FANOUT/2); one ptr = k*MIN contiguous region with log2(MIN)
      bits repurposed for length.

New Roadmap (L272)
  - final adaptive version too complex => try something reachable first.
    Preliminary: unsigned indexes allowed, block inits 0 (signed) /
    1<<(width-1) (unsigned). Planned: wrapping only after max cap +
    rotate_left(1); phys↔virt gains rotate_left(1) after max-cap split;
    negatives→odd phys, positives→even phys [I think].
  - block primitive surface => Block<Strategy>::new(); strategy determines
    find_slot/insert_before/after/addr_range; adaptive is future. Methods:
    insert_before/after, get/get_mut, cursor, remove, virt↔phys, compare.
    Block needs set_prev/set_next for ordering tags; no general arena for
    block-level users.
  · Note (L299)
    - nodes can have children at insert (root); ordering as generic arg =>
      block does wiring per ordering (bfo: parent+sibling; unordered: append;
      dfo: parent+optional child). Need an abstract tree interface to maintain
      ordering automatically. Right now: Unordered + Manual.
    novel: ordering as a generic parameter that parametrizes insert's required
      args; abstract-tree auto-wiring.

Pseudocoding (L315)
  - walker => front-and-center arena interface; consumer-supplied walker needs
    lineage remapped before returning.
  - block-level balancing => only alterations that statelessly compute updated
    ptrs; returns (block_id, ptr_range, Transform).
  - sequencing => impl fixed strategies first, adaptive capstone later.
  - walker seek-by-PTR => PTR must be Ord (not default); block_id→OrderingTag
    (u64): first=1<<63, append/prepend=±1<<32, insert=mean of neighbors.
  - are vptrs worth it? => stable across grow/spread, NOT across shifts (the
    big one), not across append/prepend splits without wrapping. [open]
  - append split: why halve? => just make a new node on the right; half-split
    only pays for mid-inserts.
  - random split: preserve negatives while interleaving positives via wrap =>
    use rotation (not shift): rightmost bit↔leftmost; contract "physical
    ordering implies logical ordering"; rotate from the start; wrapping only
    when cap ≥ PTR::max-PTR::min.
  - consistent wrapping with non-wrapping? => constrain wrapping to address
    translation on max_cap blocks; at max cap VecDeque→Vec. Translation:
    (virt+offset).shr(shift).rotate_left(rot) (+as_halfptr() cast if OVERP).
  novel: OrderingTag u64 encoding; Transform return type; rotation-as-wrap;
    "physical ordering ⇒ logical ordering" contract.
  · manual b+tree (L403)
    - b+tree over blocks; block split = tree split growing upward => subtree
      stores root addr; need insert-at-fixed-position + find_slot that won't
      cross it; root at vaddr 0 (signed) / MAX/2 (unsigned).
    - leaf arena needs wider ptrs than (U,I) => overprovisioned parent has a
      child leaf arena storing far more items in the parent's unused address
      space; eliminates reverse ptrs (parent calc'd from child+strategy). Leaf
      cap=Parent*M, grow M exponentially; terminal btree keeps M even.
    - leaf mapping for mixed pluripotent => (src_phys-first_terminal_inode)*M
      ..+M (BFS offset skips non-terminal inodes). "Dual-block": two node
      types with intertwined address spaces; insert logic totally different.
    novel: dual-block (intertwined inode/leaf address spaces);
      parent-calcuable-from-child eliminating reverse ptrs; BFS physical offset
      skipping non-terminals; overprovisioned child arena > parent capacity;
      BForest/ForestTree sketch.

Review (L508) — code review.
  · Block Module (L510)
    - try_insert_at takes a physical addr a caller shouldn't know => fix:
      before/after take virtual and map it.
    - strategy mem::take'n then handle_insertion => over-engineered for the
      block API (strategy change is the arena's job, on NotFound).
  · Strategy Module (L522)
    - insert budget is const => uncertain; auto consumer doesn't care, strategy
      would, block consumer might. [open]
    - assert_cap_pow2 overhead => debug-asserts for now; better: Pow2<T>
      newtype guaranteeing it statically.
    - InsertDelta incomplete => readdress flow = find_slot→fail→'address
      exhaust'; missing a shove variant (item pushed block→block).
    - strategy at the wrong level => fix: block stays primitive
      (insert→operate&feedback); arena.insert reads strategy→operate→feedback.
      Strategy moves to arena tier.
    - impl Block Strategy (L542): each stores budget (settable), handles
      insert/remove/post_insert_check; handle_removal has a default impl.
    - handle_removal (L549): early-return if dense or removed slot aligned;
      else shift a neighboring aligned Some toward the hole to free a None.
    - Growth Strategy (L557): each struct independently impls new_block(cap)
      => fix: move onto the growth strategy.
    - Default handle insertion (L562): all strategies just call this;
      append/prepend→push; prepend underdeveloped (no stride None insert);
      FoundAt(phys) does no shifting => fix: make symmetrical.
    - Post insert check (L571): no-op => probably unnecessary.
    - Comments (L575): many wrong/useless, some good.
    novel: Pow2<T> newtype; shove variant in InsertDelta; strategy relocated to
      arena tier.

Interfaces (L578)
  - block vs arena surface => block: try_insert_before/after, split_end/
    split_mid, shift, spread, remove, get, + range/cursor/iter (last 4 "not
    sold on"). arena.blocks[i] = raw; arena wraps with auto-handling + a queue
    of last ~16 insert hints. Arena: insert_before/after, remove, get, iter
    (fwd/rev), cursor, range.
  - nontrivial insert cases & remedies => a decision tree: block full (auto:
    split by strategy; manual: reject); out of addresses (append/prepend
    mismatch); not-found-in-budget (dense region, strategy misalignment);
    misses (floaters at hot end→shove; sequential mid-inserts→split pluripotent
    out of the middle). Per-strategy remedies for append/prepend/random/
    pluripotent + out-of-address-space cases.
  novel: insert-hint queue (last ~16) for pattern detection; density inversion
    (sparse↔dense, growth=len-occupancy) & hole-punching as remedies; the full
    per-strategy failure→remedy tree.

An idea about nibble tries integration (L683)
  - how do nibble tries fit => (1) single-block-subtree structures = "forests";
    (2) preorder ⇒ leftmost descent = linear walk while prefix_len decreases;
    (3) inodes store u8s, leaves flattened storing (block_id)→0; (4) leaf-vs-
    internal by relative position (preorder can't point back, ptr≤current ⇒
    leaf); still need 'terminal'; enforcing leaf=current for terminals too
    stingy; 4 leaves + parent ptrs in an fnode (~20B, branch/hop) fits an inode.
  novel: "forest" naming; preorder⇒leftmost-descent=linear walk;
    relative-position leaf detection; leaves cohabitating with inodes to save
    a hop.
  · DOA Block_Idx type needs sign (L694)
    - unsigned can't represent a prepended addr (0 already given, new must be
      -1) => go back to signed, just not with wrapping.
  · NonGrowing Blocks (L699)
    - lookup perf vs resize => start at max size; no v_offset/addr_shift on
      lookup (tracked only on insert to decide placement). Better lookup,
      aggressive memory; repoints still on splits; poor iteration for low-fill
      random. (half-jokes: call it "SOA" not "DOA".)
    novel: NonGrowing variant — lookup has no translation math, addressing
      state only informs insertion.
  · Find Slot (L705)
    - address vs capacity limit; bounding the search => address limit = hard
      wall; capacity limit = push_back/front. Search = aligned ± stride*budget;
      precheck append/prepend exhaustion separately. Block uses isize/usize but
      enforces handed-out ptrs representable as PTR. Branch-light: right =
      aligned+1..align(PTR::max).step_by(stride).take(budget).position(is_none);
      left mirrored, skip(1) if v_offset==PTR::MAX+1.
    novel: prechecking append/prepend exhaustion separately; branch-light
      outward scan with take(budget).

Lookup math (L742)
  - VecDeque instead of circular, preserve addrs across push_front => store
    negative count; v_offset tracks it; push_front 3×→v_offset=3, expected
    vaddr=-3. lookup(virtual)=buf[(virtual-v_offset)>>addr_shift]. No pushing
    past ptr::max (needs repoint = "address wrapping"); repoint = move v_offset
    to a big negative, tell ptrs to add it. Alt: u16s starting offset 32768.
  - per-strategy starts => append 256, prepend 65380, random 32768.
  novel: per-strategy v_offset starting points; "address wrapping
More ideas (L765)
  - 0th position as root => 0th can hold the root (nothing points to it);
    optional optimization.
  - what does insert return => InsertDelta enum: Moved(new,amount,dir),
    BShiftLeft/BShiftRight (bias change Sequential↔Random, whole block),
    BlockSplit(left_block_id,last,right_block_id).
  - how does the arena readdress self-pointing items => arena given a fn
    yielding &mut[PTR] to internal ptrs → handles readdresses/shifts
    internally. Or a block_iter the consumer shifts.
  - limit split-time repoint cost => 1 block ptr per subtree sized to fit 1
    block ⇒ split updates 1 ptr not block_size. Needs a real circular array
    (no repoint on split) — strong motivator for circular.
  - avoid repoint entirely? => eat n² insertion, make a new block on address
    exhaustion; try_insert commits only if free, else returns the needed
    operation non-committing.
  novel: InsertDelta with BShiftLeft/BShiftRight; arena-internal readdress via
    &mut[PTR] extractor; 1-block-ptr-per-subtree capping split repoint cost;
    non-committing try_insert.
  · Utility Functions (L809)
    - remap_internal / update_parents / update_children (need extractor or
      parent ptrs). Key invariant: if the only inbound ptr to a block is its
      0th element, insertions can't affect other blocks ⇒ cheap strategy
      switch ⇒ motivates dropping the nonzero-ptr requirement. Cursor must
      refuse to cross 0 (or a move_root flag) so the root/min stays put for
      linked lists. arena.blocks.split(new_root vaddr); arena.insert vs
      block.insert vs block.try_insert (try_insert doesn't split, errs).
  · Ways this might be used (L839)
    - mode A: nodes store (block_id,arena_idx), don't care about boundaries,
      readdress expensive (prefer stubs unless parent ptrs), lose the one-block
      invariant. mode B: only roots store block_id, descendants borrow it,
      control splitting, readdress via extractor, optional parent ptrs.
  · Pseudocode (L850) — Arena{read,insert,iter,blocks};
    Block{insert→Enum(Moved,Readdressed,Split), get, get_mut, remove,
    try_insert, try_insert_fixed_root, force_split(at)}.
  · Readdressing strategies (L869)
    - random→append => pack left, addr_shift=0, new right append block as a
      stub (no readdress); move the few left elements right (~7 moved frees
      7/8). But altering addr_shift needs a repoint or a move; moving vaddr
      needs a repoint anyway.
    - append→random => unavoidable repoint (massive addr_shift↑). Mitigation:
      append periodically inserts a None at the end so a stray random insert
      is unlikely to trigger a shift; none_stride=16 ⇒ open slots 16-aligned.
      Fallback: addr_shift+1 + spread on cap double (repoint+spread, costly).
    - repoint vs make-new => small block → readdr/switch; large → make new
      (random→sequential only), or split in 2 + spread both (big repoint but
      left half avoids future ones). try_insert = free→commit else non-
      committing err.
    - readdress ergonomics => hand consumer "this block changed strategies,
      here's a remap fn"; presumes cheap ordered iteration over inbound ptrs
      (consumer's problem); if they only point to root, fn is a no-op.
    - pluripotent recap => addr_shift+log2(cap)=ptr_width/2; exhausts at
      ~1-2*sqrt(MAX) elements then picks a strategy; captures overprovision
      (i32 stays pluripotent, adjusts addr_shift on spread, never readdr).
      append/prepend are the same (first+last+wrapping add, repurpose rev as
      2nd half via v_offset, 1 None every ~slots, addr_shift=0).
    novel: append's periodic None-padding absorbing stray random inserts;
      none_stride=16 16-aligned open slots; per-block "changed strategies,
      here's the remap fn" contract.

Refinement v3 (L947)
  - naming => technically "Dense" not sparse/continuous.
  - insert semantics => insert(hint) places so ordering is preserved at that
    position; returns iter<ptr>{left_end,left_start,right_end,right_start,
    current}.
  - adaptive structure + logical addressing => blocks start random-optimized;
    repeated append/prepend shifts layout+addressing to favor it. Stride =
    256>>log2(cap); cap↑⇒stride↓ to use between-addresses ("address scale"?).
    Block<PTR>{capacity,addr_shift}; new() cap=1, addr_shift=PTR::width();
    grow(append)→cap<<=1, halve addr_shift via trailing_zeros (linearizes in
    4-5 steps, needs remap); grow(random)→addr_shift>>=1. Invariant
    log2(cap)+shift=ptr_width.
  - when shift changes, what moves => (0) grow smooshes left (mass repoint
    unless no collisions; preemptive repoint when len<cap/2 to vacate parity);
    (1) shift shrinks as cap grows→spread (phys=old*2, repoint ptr>>delta, or
    nothing if extra address space); (2) shift same+cap grows→nothing; (3)
    shift shrinks more than cap grows→shift ptrs by excess. Invariants:
    virt→arr[phys] stable, max_block_size≥cap, log2(cap)+shift≤ptr_width,
    log2(max_cap)<ptr_width.
  - dodge repoint on exhaustion => overprovision the ptr or limit block size
    to a smaller ptr's max (u8 max with u16 ptr: start shift=8, append freely,
    indexes 0,256,512,…).
  novel: insert(hint)→affected-ptr iter; sqrt-pace addr_shift linearization;
    preemptive repoint when len<cap/2 to vacate parity; overprovision-or-limit
    to dodge repoint; "address scale" term.
  · Triggering Adaptive Shift (L1046)
    - when to adapt => track appends since realloc; if >cap/2, realloc/split
      favors append. Policy: blocks fill to block_max; initializers bias a new
      arena; blocks inherit parent bias. Adapt toward random if appends rare
      AND log2(cap)+shift<ptr_width-1 ⇒ repoint new=old<<delta_shift.
    novel: append-count-since-realloc as the adaptation trigger.

Refinement (v2) (L1058)
  - shape => semicircular array: like circular but begin≤0≤end; uninit between;
    signed→unsigned→address; generic over max cap; doubles when |begin|+end≥cap;
    sparse by design; first/last→terminal non-nones. Then: maybe simpler as 2
    vecs (fwd/rev, negatives inverted into rev, prepend=push to rev).
  · Insert semantics (L1073)
    - insert defaults to insert-before; insert(len) valid. Scan left by stride,
      push back, return spot + update ptrs. Crux = updating the pointing
      structure. Options: (ptr,&[(ptr,ptr)]) new-first; take a cursor, modify
      internally; iter over altered ptrs (start,dir,amount) — liked. But
      closest-open-left breaks "new takes old's spot" ⇒ insert(hint)->(ptr,
      iter<ptr>), or a cursor<ptr> with dir+amount (sufficient but shift-only,
      not spread; full iface needs a double-ended iter).
  · Layout Strategies (L1099)
    - population determines address space + distribution. Push-only→never
      spread, only realloc; on realloc check how. Trees inherit parent
      address/null stride? append/prepend realloc→null_stride↑, addr_stride↓;
      mid-insert→opposite. null_stride=1 None every null_stride on spread
      (might not use). addr_stride: phys=cap/addr_stride; cap doubles⇒phys
      doubles unless addr_stride doubles. Leave new memory for append/prepend
      ⇒ don't increase addr_stride; spread+realloc ⇒ do increase it to gap
      physical memory without repoint. Start addr_stride at max, adapt fast on
      appends (sqrt+repoint when >16), re-increase aggressively from
      mid-inserts; append/prepend only optimized if 99% of load. Monotonic-key
      btree can leave half empty on split (per-node, rebalancing handles edge).
  novel: semicircular array (begin≤0≤end); 2-vec fwd/rev alternative;
    altered-ptr iterator (start,dir,amount) as insert's return; null_stride vs
    addr_stride distinction; "append/prepend only if 99% of load";
    monotonic-key btree leaves half-empty on split.

sparse ordered arena (L1148)
  - shape => double-ended, 2 vecs (negatives inverted into rev), PTR
    const-generic, 0 invalid (Option<Nonzero<PTR>>), realloc threshold +
    target_density. Out of space at one end→new tail vec (both fwd/rev).
    Problem: realloc/spread driven by a single insert, not balanced. Fix:
    trigger spreading after traversing N elements without finding space;
    begin/end inserts first try shifting a start/end ptr.
  novel: tail vecs per end; spread-triggered-by-traversal-count (not
    per-insert); target_density.

BLOCKS (L1161)
  - arena shape & ptr encoding => arena=Vec<Block>; ptr=top-half block_idx +
    low-half item_idx (halfptrs). Block={Vec<buf>,first,last,beginning,end};
    max cap=halfptr::max; full signed range incl 0; negatives only on prepend
    (avoid shifting subsequent ptrs). Realloc on exhaustion doesn't waste
    memory (block won't grow, just make a new block).
  - append/prepend/insert => append→end↑ (unless hitting L/beginning);
    prepend→beginning↓ (unless hitting 0/end); insert→from a position skip by
    free-stride both directions (disperse puts vacancies on odds, skip evens).
  - grow vs spread => grow doubles block size+begin+end (first/last→elements
    not bounds); spread without growing if usable space doubles by moving
    begin/end; spread triggered when no space within len/cap-based bounds;
    grow when first/last need to move but overlap.
  - adaptive stride => block stores stride (default 1, max ~16); realloc from
    append/prepend→stride+1; mid-insert→stride-1; spread inserts 1 None every
    stride. Keeps a dense append-made block at 15/16 instead of flooding it
    with Nones from one mid-insert. Cap always doubles on grow.
  - cascading shifts => arena=Vec<(prev,Block,next)> linked list so a new
    block doesn't cascade; list only for iteration, ptrs still use arena index.
  novel: halfptr ptr encoding (top=block, low=item); adaptive stride 1..16
    (+1 append/prepend realloc, -1 mid-insert); negatives-only-on-prepend;
    linked-list arena avoiding cascading shifts; spread-without-grow by moving
    begin/end.
  · insertion (L1191) — items aren't Ord (leafnodes), caller requests space +
    provides iter_mut<&mut PTR> for remapping. len=0→just push. Look outward
    first (symmetric, better expansion). Multi-ptr-per-item⇒2-level iter;
    single-ptr-per-item guarantee⇒1-level.
  · Splitting (L1203) — blocks don't overflow into neighbors (no benefit); new
    block only when previous can't split further; append/prepend-made→dense
    (cap≈len); random-made→leave space. Split: new block, repoint half the
    ptrs, copy items (prepend→first half, append→second, spread→second+spread
    both), wire the list.
    prepend→mirror, else balanced begin=-end); detect bias by stride or
    |end|≫|begin| (append-heavy, stride>3).
  · iteration (L1222) — no mod per index; 1-2 loops with usize start/end
    (ustart=if start>0{start}else{len+start}); is start%len equivalent when
    len is pow2? Iterate blocks then items till next/prev==None. No-aliasing
    policy (each item ≤1 inbound ptr); keys the exception. 2d-iter as
    &[&mut PTR].
  · Stable Indeces on Repeated Spread (L1241) — stable indices via ptr>>log2
    (cap) (64>>6=1). Downside: odd unfilled→even unfilled in new buffer; 1
    extra bitshift/lookup; hurts append/prepend-heavy ⇒ maybe only when
    stride=1 growing from spread; spread default 3 so it takes 2 spread-grows
    without append/prepend to take over. Maybe need
    append_grow/prepend_grow/spread_grow.
  · circular (L1269) — restrict begin≤0: phys_idx=if i<0{len+i}else{i}.
  novel (subsection): outward-first insert; single-ptr-per-item⇒1-level iter;
    PTR=(u16,i16) tuple; bias detection via stride/|end|:|begin|; ptr>>log2
    (cap) stable indexing; spread_default=3.
        (saves space for big leaves); header = union of PartitionHeader/Leafnode,
        no discriminant, no branch.
      novel: uBlock growing by neighbor-borrowing; per-strategy side-array of
        nonzero u8s; PartitionHeader/Leafnode union; "partitioned only with
        pluripotent."
    · Leafblock (L191)
      - discrete type or inside block? => separate for now (interface too
        different from block's).
      - where does the header live? => options: inl