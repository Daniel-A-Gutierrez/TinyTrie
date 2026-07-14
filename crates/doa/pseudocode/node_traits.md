```rust
// Node traits — how the arena traverses the consumer's structure to remap
// pointers after a move, instead of handing the whole ArenaInsertDelta back
// for the consumer to apply. The consumer exposes its pointer slots via small
// traits; the arena does the remap in place.
//
// Ptr is the arena-interface pointer: (block_id, in-block addr). Both halves
// are Eq+Copy via the index traits (UnsignedIndex/SignedBlockIndex bound Eq,
// Copy), so the walker copies Ptrs out for navigation and scans slots by value.
type Ptr<U, I> = (U, I);

// =====================================================================
// The axis: does the node store a parent pointer?
//
//   YES  -> inptr. From each moved node, follow parent()/children()/sibling
//          pointers DIRECTLY to the holder of each slot pointing into it, and
//          rewrite that slot. No walker, no lineage arg, no preorder
//          assumption. (insert_tree; BidirGraphNode is the graph general form.)
//   NO   -> outptr, needs a walker. The node can't reach its own holders, so
//          the arena walks the structure to enumerate them. Caller supplies
//          the lineage (path to the run's first node); preorder assumed so a
//          contiguous run's holders are local to it. (insert_lineage.)
//
// The walker assumes T stores NO parent pointer. Parent-pointer nodes never
// use the walker — they use the direct inptr method instead.
// =====================================================================

// ---------------------------------------------------------------------
// Slot-exposing traits — what pointer fields a node stores.
// Read accessors drive navigation; &mut accessors drive in-place rewrite.
// All yield PRESENT pointers only — the node hides sparse/Option storage.
// Plain iterators: navigation copies Ptrs out by value into a local buffer,
// then walks the buffer; the live accessor is only ever drained once to fill
// it, so it need not be double-ended.
// ---------------------------------------------------------------------

trait TreeNode<U: UnsignedIndex, I: SignedBlockIndex> {
    fn children(&self)         -> impl Iterator<Item = Ptr<U, I>>;
    fn children_mut(&mut self) -> impl Iterator<Item = &mut Ptr<U, I>>;
}
trait StoresNextSibling<U: UnsignedIndex, I: SignedBlockIndex> {
    fn next_sibling(&self)     -> Option<Ptr<U, I>>;
    fn next_sibling_mut(&mut self) -> &mut Option<Ptr<U, I>>;
}
trait StoresPrevSibling<U: UnsignedIndex, I: SignedBlockIndex> {
    fn prev_sibling(&self)     -> Option<Ptr<U, I>>;
    fn prev_sibling_mut(&mut self) -> &mut Option<Ptr<U, I>>;
}
// Used by inptr, NOT by the walker.
trait StoresParent<U: UnsignedIndex, I: SignedBlockIndex> {
    fn parent(&self)     -> Option<Ptr<U, I>>;
    fn parent_mut(&mut self) -> &mut Option<Ptr<U, I>>;
}
// inptr generalised to graphs — every edge reciprocated. Parallel path, not a
// TreeNode superset. Gets its own insert method; no preorder, no lineage.
trait BidirGraphNode<U: UnsignedIndex, I: SignedBlockIndex> {
    fn in_ptrs_mut(&mut self)  -> impl Iterator<Item = &mut Ptr<U, I>>;
    fn out_ptrs_mut(&mut self) -> impl Iterator<Item = &mut Ptr<U, I>>;
}

// ---------------------------------------------------------------------
// Reloc — a relocated run + its transform. (Renamed from "Chunk": this names
// what it is — where-from, where-to, how to map addrs.) Defined in
// block_management.md; restated here typed. An in-block shift is one Reloc
// with a trivial transform (offset); a split is several, possibly across
// blocks; the walker/inptr operate arena-level across them all.
// ---------------------------------------------------------------------
struct Reloc<U, I> {
    old_block: U,
    old_range: (I, I),
    new_block: U,
    transform: Linear,            // from block_management.md
}

// =====================================================================
// NodeOrdering — the Walker (outptr only; T stores no parent pointer).
// One impl per layout (Preorder, BFO, DFO). The arena builds it inside
// insert (which holds &mut self); the consumer names the ordering as a type
// parameter. The walker borrows the arena mutably and does ALL mutation
// itself — no callback, no copy-out/transform/copy-in of slots.
// =====================================================================

trait NodeOrdering<U: UnsignedIndex, I: SignedBlockIndex> {
    // Walk the moved set described by `relocs` (arena-level; may span blocks),
    // starting from `lineage` (caller-supplied path of Ptrs to the run's first
    // node). For each holder, rewrite its slots whose target falls in a Reloc.
    //
    // Per node (the contract that makes this borrow-clean):
    //   1. drain children() — copy Ptrs out BY VALUE into a local buffer
    //      (read borrow ends; the buffer is owned/frozen).
    //   2. mutate slots in place via children_mut() (+ sibling_mut if bound) —
    //      one &mut Ptr at a time, sequential, none held.
    //   3. descend through the buffer, resolving each Ptr fresh through the
    //      arena (no node borrow held during the descent).
    //
    // Navigation reads the FROZEN copies, never a live slot — so mutating at
    // visit time is safe. No pop-back, no rewrite-on-exit, no "did I already
    // rewrite this?" ambiguity: you never re-read a live slot to navigate.
    //
    // Bounded descent: descend run nodes fully. For a run node's EXTERNAL
    // child (post-boundary), visit it — rewrite its slots targeting moved
    // (parent->run-node, sibling->moved-sibling) — but do NOT descend into it:
    // its children's parent-slots target *it* (external, not moved), so they
    // aren't holders. The descent stops one level past the boundary.
    fn apply_relocs<T>(
        arena: &mut Arena<T, U, I>,
        relocs: &[Reloc<U, I>],
        lineage: &[Ptr<U, I>],
    ) where T: TreeNode<U, I>;
}

// Preorder impl — the worked case: a contiguous preorder run's holders are
// its ancestors (the lineage) + the run itself (scanned) + the run's immediate
// external children (the bounded descent). DFS per the contract above.
// BFO/DFO: different traversal shapes (BFO walks level by level), same
// accessor and same apply-in-place contract; only the per-ordering walk
// differs. Details TBD.

// =====================================================================
// Arena insert — distinct names + distinct where clauses on one impl block.
// No specialization: each is only callable when its bounds hold. The bound is
// the set of slot traits T stores, so the method can call those accessors.
// Remap happens BEFORE the physical move (transforms are old->new; the lineage
// and slots resolve against the old layout). The inbound value T's own fields
// are remapped in the same pass. Pointers TO the new node are set AFTER it is
// placed (its address is only known then) and returned in the delta.
// =====================================================================

impl<T, U, I> Arena<T, U, I>
where U: UnsignedIndex, I: SignedBlockIndex, T: Sized {

    // Pointerless T. Arena does storage + rebalancing; returns the delta for
    // the consumer to remap pointers it holds OUTSIDE the arena. The "node ==
    // key, sorted array with ~log(n) insert" case. No remap pass — T stores no
    // internal pointers.
    fn insert(&mut self, anchor: Ptr<U, I>, value: T) -> ArenaInsertDelta

    // No parent pointer. Outptr via the walker. Caller supplies the lineage
    // (can't be walked — no StoresParent). Preorder assumed. Sibling slots are
    // rewritten if T implements the sibling traits (add them to the bound).
    fn insert_lineage<O: NodeOrdering<U, I>>(
        &mut self,
        anchor: Ptr<U, I>,
        lineage: &[Ptr<U, I>],
        value: T,
    ) -> ArenaInsertDelta
    where T: TreeNode<U, I>  // + StoresNextSibling<U,I> + StoresPrevSibling<U,I> as stored

    // Stores a parent pointer. Inptr, NO walker. Scan the moved set; for each
    // moved node, rewrite its own slots targeting moved (local), then follow
    // its reciprocal pointers to the holders of slots pointing into it —
    // parent() -> parent's child-slot, children() -> external child's
    // parent-slot, sibling ptrs -> sibling slots — and rewrite those in place.
    // No lineage arg, no preorder assumption.
    fn insert_tree(
        &mut self,
        anchor: Ptr<U, I>,
        value: T,
    ) -> ArenaInsertDelta
    where T: TreeNode<U, I> + StoresParent<U, I>  // + sibling traits as stored
}

// parallel path
impl<T, U, I> Arena<T, U, I>
where U: UnsignedIndex, I: SignedBlockIndex, T: BidirGraphNode<U, I> {
    fn insert_bidir(&mut self, anchor: Ptr<U, I>, value: T) -> ArenaInsertDelta
}

// =====================================================================
// Recap
//
// Holder set to remap = internal ∪ external.
//   internal : nodes inside the moved set — found by the arena scanning the
//              Reloc old-ranges (the arena's ordering is the reverse index for
//              a contiguous run; no trait navigation needed to find them).
//   external : nodes outside pointing in.
//              outptr (insert_lineage) -> the caller's `lineage` + the run's
//                immediate external children (bounded descent).
//              inptr  (insert_tree)     -> reached by following reciprocal
//                pointers from each moved node (no lineage, no preorder).
//              insert_bidir            -> BidirGraphNode reverse edges.
//              insert                  -> none; consumer's external ptrs come
//                back via ArenaInsertDelta.
//
// Why no for_each_ptr_mut: generic slot enumeration loses the layout order and
// invites copy-out/transform/copy-in of slots. The walker/inptr applies the
// Reloc transforms directly to the slot, in place — only navigation copies Ptr
// VALUES (cheap, by-value), never slots.
//
// The copy-out contract is what makes the walker borrow-clean: can't hold a
// collection of &mut Ptr (aliasing), can't mutate a slot then follow it. So
// drain the read accessor into owned Ptrs first, mutate in place, navigate via
// the owned copies. The node is borrowed only for drain+mutate.
//
// Reloc / Linear / ArenaInsertDelta defined in block_management.md
// (authoritative); remediation flow producing them in arena_insertion.md.
//
// Open: cursor API to produce `lineage` from a hover position. BFO/DFO walker
// traversal shapes. BidirGraphNode in_ptrs_mut lending-borrow shape (likely a
// callback, not an iterator). Exact bound composition per insert method (the
// slot traits T stores) — distinct methods per combo vs one self-describing
// trait.
```