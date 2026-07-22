```rust
// Node traits — how the arena traverses the consumer's structure to remap
// pointers after a move, instead of handing the whole ArenaInsertDelta back.
// The consumer exposes its pointer slots via one self-describing trait; the
// arena does the remap in place.
//
// Ptr = (U, I): (block_id, in-block addr). Eq+Copy via the index traits.
// BLOCK-ID SPACE: each Arena owns a VecDeque<Block>; block_id is a per-arena
// index. A Ptr is disambiguated by ROLE CONTEXT (the consumer knows, by
// structure position, which arena a Ptr targets), NOT by a tag. Consequence:
// Reloc sets from different arenas MUST NEVER be merged (block_ids overlap).
// The arena never merges them — each insert returns its own delta; the consumer
// applies them per-arena. If you want a tag-free global space, use ONE arena.
type Ptr<U, I> = (U, I);

// ---------------------------------------------------------------------
// Order-maintenance labels — block linked-list order, O(1) compare.
// block_id is a STABLE append index (append-only VecDeque) and is NOT the
// block's list position. The arena keeps a side table `tags: Vec<u64>`,
// `tags[block_id]` = the block's order in the linked list. First block tag
// 1<<63; sequential append/prepend step 1<<32; an insert-between (split /
// new_block_between) takes the midpoint of the siblings' tags. Compare two
// block_ids by tag in O(1); relabel a short run only on adjacent-tag overflow
// (TODO). This backs Arena::cmp_ptr — the key-free descent primitive: search a
// tree whose child Ptrs ARE the keys, without ever seeing a key value. Same
// block -> addr compare (virt is monotonic in sequence position); else tag.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// Remap — the consumer-facing surface of "what moved and where", boiled down
// from ArenaInsertDelta. One struct + one INFALLIBLE method, no dyn, no heap.
// Every remediation move is a Transform over a chunk {old_block, old_range,
// new_block}; arity bounded <=3 (Free=0, Shifted/Readdressed=1, Split<=3,
// Shoved<=2), cascades recursive -> fixed enum, no Vec. `map` is infallible:
// the guard is identity, not None — the consumer applies every Remap in the
// set to every slot; out-of-range -> identity, order irrelevant, no routing at
// the call site. The arena builds the set, so it knows it is correct.
//
// Reloc / Linear (block_management.md) are the arena's PRIVATE shape, expressed
// in phys where convenient; the arena condenses them to RemapSet in virt/Ptr
// for the consumer. The consumer never sees phys or Linear.
// ---------------------------------------------------------------------
enum Transform { Left { shift: u32, offset: isize }, Right { shift: u32, offset: isize } }
impl Transform {
    // new = (old << shift) + offset | (old >> shift) + offset  (Right lossless for live addrs)
    fn map(&self, addr: I) -> I;
}
struct Remap<U, I> { old_block: U, old_range: (I, I), new_block: U, transform: Transform }
impl<U, I> Remap<U, I> {
    // identity if p not in (old_block, old_range); else (new_block, transform.map(addr))
    fn map(&self, p: Ptr<U, I>) -> Ptr<U, I>;
}
enum RemapSet<U, I> { None, One([Remap<U,I>; 1]), Two([Remap<U,I>; 2]), Three([Remap<U,I>; 3]) }

// Reloc — the arena's INTERNAL relocated run + transform (block_management.md
// now uses this name too). An in-block shift is one Reloc with a trivial
// transform; a split is several; arena-level passes iterate them all. Condensed
// to RemapSet only on the pointerless path; on the walker/revptr paths the
// walker consumes Relocs directly to rewrite in-arena ptrs in place.
struct Reloc<U, I> {
    old_block: U,
    old_range: (I, I),
    new_block: U,
    transform: Linear,            // block_management.md
}

// ---------------------------------------------------------------------
// ArenaNode — the self-describing slot trait. A node implements ONLY the slot
// kinds it stores; the rest keep their default (empty / None). The remap calls
// every mut accessor and skips the None ones, so it rewrites whatever the node
// stores — no combinatorial bound per slot set, no specialization.
//
// Read accessors drive navigation; &mut accessors drive in-place rewrite.
// All yield PRESENT pointers only — the node hides sparse/Option storage.
// Plain iterators (drained once into a buffer for navigation): see copy-out.
// ---------------------------------------------------------------------
trait ArenaNode<U: UnsignedIndex, I: SignedBlockIndex> {
    fn children(&self)       -> impl Iterator<Item = Ptr<U,I>> { iter::empty() }
    fn parent(&self)         -> Option<Ptr<U,I>> { None }
    // Lateral links: a forward/backward Ptr the node stores. The framework does
    // NOT wire these (no LINK step); the consumer splices the new node and
    // maintains these themselves. A tree's right sibling, a B+ leaf's chain
    // next, a trie's peer — all are a next_link to the framework. parent/children
    // it distinguishes (cross_arena follows one or the other); lateral links it
    // does not.
    fn next_link(&self)   -> Option<Ptr<U,I>> { None }
    fn prev_link(&self)   -> Option<Ptr<U,I>> { None }

    fn children_mut(&mut self)   -> impl Iterator<Item = &mut Ptr<U,I>> { iter::empty() }
    fn parent_mut(&mut self)     -> Option<&mut Option<Ptr<U,I>>> { None }
    fn next_link_mut(&mut self)  -> Option<&mut Option<Ptr<U,I>>> { None }
    fn prev_link_mut(&mut self)  -> Option<&mut Option<Ptr<U,I>>> { None }
}
// A node with only forward child pointers implements children/children_mut and
// leaves the rest default — it has no reciprocal edges, so it uses the walker
// (insert_lineage). A node with parent + lateral link pointers implements those
// too and uses revptr (insert_tree). The method choice is a CONVENTION tied to which
// read accessors return Some, not a static bound: insert_tree requires
// parent() to be meaningful for every node; else use insert_lineage.

// BidirGraphNode — parallel path for graph-shaped consumers (every edge
// reciprocated; holder discovery via reverse edges, no preorder, no lineage).
// Not needed for trees; kept as an extension. in_ptrs_mut is a plain chained
// iterator over the node's reciprocal slots (no callback needed).
trait BidirGraphNode<U: UnsignedIndex, I: SignedBlockIndex> {
    fn in_ptrs_mut(&mut self)  -> impl Iterator<Item = &mut Ptr<U,I>>;
    fn out_ptrs_mut(&mut self) -> impl Iterator<Item = &mut Ptr<U,I>>;
}

// =====================================================================
// The two remap mechanisms
//
// REVPTR (no walker) — T stores a parent (reverse) pointer. From each moved node, follow
//   its reciprocal read accessors (parent/children/links) directly to the
//   holder of each slot pointing into it, rewrite that slot in place. No
//   lineage arg, no preorder assumption. O(moved x reciprocal-degree).
//
// WALKER (no reverse ptr) — T stores no parent pointer, can't reach its holders. The
//   arena walks the structure to enumerate them; caller supplies the lineage
//   (the NEW NODE's ancestors — path from root to where the consumer is
//   inserting), NOT the moved run's first node. Preorder assumed so a
//   contiguous run's holders are local to it. The walker rewrites every
//   in-arena ptr in place, so insert_lineage returns just the new Ptr — no
//   RemapSet; the consumer doesn't touch pointers. NodeOrdering is one impl
//   per layout.
// =====================================================================

trait NodeOrdering<U: UnsignedIndex, I: SignedBlockIndex> {
    // Walk the moved set (relocs, arena-level, may span blocks) from `lineage`
    // (the new node's ancestors). `lineage` is OWNED data — a path of (Ptr,
    // child_idx): each entry is an ancestor and the index of the NEXT entry
    // among that ancestor's children. The child_idx lets apply_relocs locate
    // the holder slot O(1) (parent.children[child_idx] is the moved node) instead
    // of scanning children(). No borrow on the arena is held in the path, so the
    // arena can be taken &mut alongside it; the consumer builds the path with
    // their own descent (cmp_ptr / cursor / whatever) and hands it in.
    // Per node, ITERATE its slots via the mut accessors and apply each Reloc's
    // transform in place through UNSAFE DISJOINT BORROWS of discrete arena
    // points (one &mut T per distinct (block, addr), via UnsafeArena::get_mut).
    // No copy-out, no freeze, no drain — the live slots are mutated directly.
    // Sound under two consumer guarantees:
    //   - NON-SELF-REFERENTIAL: a node never stores a Ptr to itself (else a
    //     slot and the node it lives in alias).
    //   - NO INTRA-NODE ALIASING: a node never stores two Ptrs to the same
    //     other node (else two &mut Ptr in one node alias).
    // Cross-node aliasing (one node held by several holders) is harmless:
    // Remap::map is a pure fn of the slot value, no per-ptr visited-set, so
    // rewriting the same target twice is idempotent. Bounded descent: descend
    // run nodes fully; for a run node's EXTERNAL child, visit + rewrite its
    // slots targeting moved but do NOT descend (its children's parent-slots
    // target it, not moved — not holders).
    fn apply_relocs<T: ArenaNode<U,I>>(
        arena: &mut Arena<T,U,I>,
        relocs: &[Reloc<U,I>],
        lineage: &[(Ptr<U,I>, usize)],
    );
}

// =====================================================================
// The two walks (do not confuse):
//
// MOVED-RANGE walk (arena-owned) — apply_relocs walks the remediation's moved
// set in ARENA SEQUENCE ORDER (block `next` + addr stride), O(1) advance. The
// arena owns this: it knows block prev/next + addr_stride. Snapshot-safe by
// construction: advance follows block-next/addr, which is independent of any
// node's stored links, so slot rewrites never corrupt advance. Exposed to the
// consumer as a fast sequential iterator too. This is the remap pass —
// consumer-supplied logic via NodeOrdering, but driven by the arena.
//
// STRUCTURE DESCENT (consumer-owned, NOT required to be a Walker) — the
// consumer descends its tree to find the insert point and BUILD the lineage
// path. The arena cannot do this (it doesn't know the tree's edge semantics),
// so the consumer does it however they like: cmp_ptr key-free descent, a
// cursor, their own walker, anything. The ONLY thing that crosses the seam is
// the resulting owned path `&[(Ptr, usize)]` (ancestor + child_idx). So the
// consumer does NOT have to use an arena-provided walker to traverse — they
// hand the path, not a live traversal object. (A borrow-free owned Walker could
// be the path type; a plain slice works too. A live Walker that holds &arena
// would fight the &mut arena the insert needs — avoid it; resolve Ptrs through
// the arena on demand instead.)
//
// CONTRACT the consumer's NodeOrdering impl must uphold during the moved-range
// walk (red-letter):
//   - At each node X, mutate only X's HOLDERS (slots pointing AT X), never X's
//     own slots. X's own slot targeting a co-moved successor S is rewritten when
//     visiting S (X is a holder of S), never while on X.
//   - Advance ONE direction; read the forward link at advance time. The only
//     holder-mutations that could touch that link occur on LATER nodes (after
//     passing X), so it is safe. Do NOT rewrite the current node's own forward
//     link before advancing — remap never does; consumer extra wiring must too.
// =====================================================================

// =====================================================================
// Arena insert — place + remap moved; CONSUMER WIRES. The arena only READDRESSES
// (changes Ptr values); it never REWIRES (changes who points to whom). No LINK
// step: a freshly inserted node has no children yet, we don't know where in the
// parent the consumer wants the new child, and prev/next may mean something
// other than arena-order (skip lists). All wiring (parent's child slot, child's
// parent-back, lateral links) is the consumer's job. Remap BEFORE the physical
// move (transforms are old->new; lineage and slots resolve against the old
// layout). The inbound value's own fields are remapped in the same pass.
//
// What the consumer gets back depends on the path:
//  - insert_lineage (walker): the walker rewrites every in-arena ptr in place;
//    returns just the new Ptr. No RemapSet — the consumer doesn't touch ptrs,
//    and has no knowledge of any arena rebalancing that happened during place.
//  - insert_tree (revptr): reciprocal in-arena slots rewritten internally;
//    returns just the new Ptr. No RemapSet — same reason.
//  - insert (pointerless): T stores no in-arena ptrs at all; every consumer-held
//    ptr is out-of-band -> returns (new_ptr, RemapSet) for manual remap.
//
// Borrow: the walker/lineage path resolves Ptrs itself and accessors only touch
// &mut self of one node at a time -> UNSAFE DISJOINT BORROW is the chosen path:
// UnsafeArena::get_mut(block, addr) -> &mut T, one live &mut per distinct
// (block, addr), no copy-out, no freeze. Sound under two consumer guarantees:
//   - NON-SELF-REFERENTIAL: a node never stores a Ptr to itself.
//   - NO INTRA-NODE ALIASING: a node never stores two Ptrs to the same other
//     node. Cross-node aliasing (one node held by several holders) is harmless
//     — Remap::map is a pure fn of the slot value, no per-ptr visited-set, so
//     rewriting the same target twice is idempotent.
// The revptr path chases reciprocal edges inside an accessor -> needs &mut
// arena there -> copy-out (drain to owned ints, drop borrow, reborrow per
// mutate) is the fallback for that path.
// =====================================================================

impl<T, U, I> Arena<T, U, I>
where U: UnsignedIndex, I: SignedBlockIndex, T: Sized {

    // Pointerless T. Arena does storage + rebalancing; returns the new Ptr +
    // RemapSet for any pointers the consumer holds OUTSIDE the arena (out-of-
    // band per-block metadata the framework can't see). No remap pass inside T
    // — it stores no in-arena pointers. The "node == key, sorted array with
    // ~log(n) insert" case. (The only path that returns a RemapSet.)
    fn insert(&mut self, anchor: Ptr<U,I>, value: T) -> (Ptr<U,I>, RemapSet<U,I>)

    // REVPTR. T stores parent pointers (parent() meaningful for every node).
    // Scan the moved set; for each moved node rewrite its own slots targeting
    // moved (local), then follow reciprocal read accessors to the holders of
    // slots pointing into it and rewrite those. Returns just the new Ptr — no
    // RemapSet; reciprocal in-arena slots are rewritten internally, and the
    // consumer wires the new node into the tree themselves.
    fn insert_tree(&mut self, anchor: Ptr<U,I>, value: T) -> Ptr<U,I>
    where T: ArenaNode<U,I>

    // WALKER. T stores no parent pointer; caller supplies the lineage (the new
    // node's ancestors). Preorder assumed. The walker rewrites every in-arena
    // ptr in place via apply_relocs, so this returns just the new Ptr — no
    // RemapSet. The consumer wires the new node; it has no knowledge of any
    // arena rebalancing that happened during place.
    fn insert_lineage<O: NodeOrdering<U,I>>(
        &mut self, anchor: Ptr<U,I>, lineage: &[(Ptr<U,I>, usize)], value: T,
    ) -> Ptr<U,I>
    where T: ArenaNode<U,I>
}

// parallel path
impl<T, U, I> Arena<T, U, I>
where U: UnsignedIndex, I: SignedBlockIndex, T: BidirGraphNode<U,I> {
    fn insert_bidir(&mut self, anchor: Ptr<U,I>, value: T) -> (Ptr<U,I>, RemapSet<U,I>)
}

// =====================================================================
// Cross-arena insert — "what moved" and "what points" in different arenas.
//
// Relocations come from the pointee arena; the rewrite (fix any slot, in either
// arena, whose target moved) doesn't care which arena the holder lives in.
// Single-arena insert_tree is the special case where holders live in `self`.
// Requires role-contextual Ptr resolution (above) or a unified space, and Reloc
// sets never merged across arenas.
//
// MECHANICS. cross_arena_insert = insert_tree on the pointee arena (place +
// same-arena remap) PLUS cross-arena reciprocal rewrites. For each node the
// pointee reorg MOVES, the framework follows that node's CROSS-ARENA edge into
// pointer_arena and rewrites the counterpart slot. The edge goes two ways
// depending on which kind of node moved:
//
//   via PARENT (the moved node's parent() crosses): follow parent() to its
//     HOLDER in pointer_arena; rewrite the holder's slot that targeted the moved
//     node — scan the holder's children_mut for the OLD Ptr, set the NEW.
//     Example (b_tree.md): a leaf moves in the leaves arena -> its parent Ptr
//     reaches the inode in the inodes arena -> rewrite that inode's child entry.
//
//   via CHILDREN (the moved node's children() cross): follow each child() to its
//     TARGET in pointer_arena; rewrite that target's parent_mut to the moved
//     node's NEW Ptr. Example: a bottom-level inode moves in the inodes arena ->
//     its child Ptrs reach leaves in the leaves arena -> rewrite each leaf's
//     parent.
//
// Both directions occur in one tree: leaf moves use via-parent; bottom-inode
// moves use via-children. Same-arena edges (a moved leaf's next_leaf; a moved
// non-bottom inode's parent/children) are handled by insert_tree WITHIN their
// own arena, not here. No LINK — the consumer wires cross-arena pointers
// themselves after the remap.
//
// THE SEAM THIS EXPOSES — which edge crosses? The framework cannot tell from a
// Ptr alone: block_ids overlap across arenas and there is no tag. Whether a
// moved node's parent or children (or neither) targets pointer_arena depends on
// the node's ROLE/LEVEL (a bottom inode's children cross; a higher inode's
// don't), which a static trait can't encode. So the caller MUST designate the
// cross edge per call. Options, undecided: the enum arg below; two fns
// (cross_via_parent / cross_via_children); or a 1-bit tag on Ptr (which would
// also dissolve the role-context caveats at the top of this file). The
// signature below takes the enum; revisit once a consumer stresses it.
//
// Remap before the pointee arena's physical move. pointer_arena is rewritten in
// place (no insert there), so no RemapSet is returned for it — only the pointee
// arena's (new_ptr, RemapSet).
//
// NOTE: the b_tree mock uses this for trait-managed cross-arena pointers
// (leaf.parent is an ArenaNode slot, not out-of-band). For genuinely out-of-band
// metadata (per-block side data the consumer stores outside both arenas),
// remap is the consumer's manual job from RemapSet — cross_arena_insert can't
// see fields that aren't ArenaNode slots.

enum CrossEdge { Parent, Children }

fn cross_arena_insert<P, E, U, I>(
    pointer_arena: &mut Arena<P, U, I>,   // holders / reciprocal targets
    pointee_arena: &mut Arena<E, U, I>,   // pointees; the insert happens here
    cross:   CrossEdge,                    // which of the pointee's edges crosses to pointer_arena
    pointee: Ptr<U, I>,                    // anchor: place `value` next to this pointee
    value:   E,
) -> (Ptr<U,I>, RemapSet<U,I>)             // pointee arena's result only
where U: UnsignedIndex, I: SignedBlockIndex,
      P: ArenaNode<U, I>,   // holder exposes the slot to rewrite (children_mut or parent_mut)
      E: ArenaNode<U, I>;   // pointee exposes the crossing edge (parent() or children())

// Cascading splits reuse this (or single-arena insert_tree) recursively: the
// separator that propagates up is a KEY VALUE (copied in B+, moved in B-tree —
// either way pointerless); only the child Ptr to the new half propagates as a
// pointer, set after placement. The consumer orchestrates split/recurse.

// =====================================================================
// Recap
//
// Arena readdresses only (Ptr values); consumer rewires only (who points to
// whom). Parentage is stable across a move; a B-tree parent-split is a separate
// consumer op (insert empty node -> arena remaps -> consumer reshuffles child
// ptrs). Return: walker and revptr return just new_ptr (in-arena ptrs rewritten
// internally); only the pointerless `insert` returns (new_ptr, RemapSet) for
// out-of-band ptrs the consumer holds outside the arena. The consumer never sees
// or acts on the arena's block-level rebalancing — that is automatic and
// transparent; block-direct users (no arena) rebalance themselves.
//
// Holder set = internal u external.
//   internal : nodes inside the moved set — found by scanning Reloc old-ranges
//              (the arena's ordering is the reverse index for a contiguous run).
//   external : insert_tree -> reached by following reciprocal read accessors
//                from each moved node (no lineage, no preorder).
//              insert_lineage -> caller `lineage` (new node's ancestors) +
//                run's immediate external children (bounded descent).
//              insert_bidir -> BidirGraphNode reverse edges.
//              insert -> none; consumer's external/out-of-band pointers come
//                back via RemapSet for manual remap.
//
// ArenaNode self-describes its slots (defaults for absent kinds) -> one bound,
// no combinatorial methods, no specialization. The remap rewrites whatever the
// node stores.
//
// Borrow: the walker path uses UNSAFE DISJOINT BORROWS (one &mut T per distinct
// (block, addr), no copy-out, no freeze) under the consumer's non-self-
// referential + no-intra-node-aliasing guarantees; the revptr path falls back to
// copy-out (drain to owned ints) since it chases reciprocal edges inside an
// accessor. Remap::map is stateless / pure -> cross-node slot aliasing is
// harmless. Can't mutate a slot then follow it.
//
// Block-id space is per-arena; Ptrs disambiguated by role context; Reloc sets
// never merged across arenas. One arena avoids the caveat entirely. Block
// linked-list order is the OM-label side table, not block_id.
//
// Remap / Transform / RemapSet: here (authoritative consumer surface).
// Reloc / Linear / ArenaInsertDelta: block_management.md (authoritative
// internal shape). Remediation flow: arena_insertion.md. Example consumer:
// b_tree.md.
//
// Open: relabel-on-overflow for OM labels (adjacent-tag midpoint). cursor API
// to produce `lineage` from a hover position. Whether lineage is a plain slice
// `&[(Ptr, usize)]` or a borrow-free owned Walker type (resolve-on-demand, no
// &arena held) — a live Walker holding &arena would fight the &mut arena insert
// needs. BFO/DFO walker shapes (Preorder is the worked one). force_split seam
// (consumer-driven split at a chosen point — see b_tree.md; uses block_management's
// split_in_two).
```