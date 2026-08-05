use crate::RelTo;
use crate::block::{BlockMutTrait, BlockTrait};
use crate::translator::Translator;
use crate::index::*;
use std::fmt;
use std::marker::PhantomData;
use crate::Ordering;
///projections from a `Tree`'s witness `Inner` — the single source of truth, so
///walker/probe agree with the block by construction (no equality bounds). `P`/`T`/`K`
///are NOT separate associated types on `Tree`; they're always these projections.
pub type TreeP<'a, T> = <<T as Tree<'a>>::Inner as BlockTrait<'a>>::P;
pub type TreeT<'a, T> = <<T as Tree<'a>>::Inner as BlockTrait<'a>>::T;
pub type TreeK<'a, T> =
    <TreeT<'a, T> as Node<'a, TreeP<'a, T>, <T as Tree<'a>>::O>>::K;
pub type TreeV<'a, T> =
    <TreeT<'a, T> as Node<'a, TreeP<'a, T>, <T as Tree<'a>>::O>>::V;

///propagated split result: the new (right) node + the separator promoted to the parent.
///carried by value up the walker's parent stack; the walker phys-places `right` (`place_new`)
///then inserts `sep` — pointing at the placed right's vaddr — into the parent via `Node::insert`.
pub struct Overflow<N: Sized, K> {
    pub right: N,
    pub sep: K,
}

///what `Node::insert` is adding alongside a key: a value (terminal node) or a child pointer
///(internal node). the caller (walker) knows the height and picks the arm; the node impl maps
///it to storage (`INode`: `Value(v) => v`, `Child(p) => PtrUnion{internal: p}`). tagged (not two
///Options) so exactly one payload is expressible — `(Some,Some)`/`(None,None)` aren't.
pub enum Payload<V, P> {
    Value(V),
    Child(P),
}


/// `'a` here and on every `*<'a, ...>` below is ONLY the outlives bound: it guarantees
/// `T` outlives the block/store/collection. It is NOT a borrow lifetime. Walker/probe
/// borrow lifetimes are a separate parameter (`'b`), constrained `'a: 'b`.
pub trait OrderedNode<P: BlockIndex, O: Ordering> {
    ///vaddr the new child at child_idx should be placed before/after.
    fn insert_position(&self, this: P, child_idx: usize) -> RelTo<P>;
}

/// blocks that store a type that impls Node support automatic internal navigation by default.
/// D = DEGREE , the maximum number of children of a node.
/// `P` is the block's ptr type (a param, not associated, so Tree::P and the node's P
/// are the SAME type in generic contexts). `'a` is the outlives bound only; accessor
/// iterators are scoped to `&self`'s own borrow (`'s`, `'a: 's`).
pub trait Node<'a, P: BlockIndex, O: Ordering>: Sized + 'a + OrderedNode<P, O> + Default + Clone {
    type K: Sized + 'a + Ord;
    type V: Sized + 'a;

    fn lookup<'s>(&'s self, query: &Self::K) -> Option<impl NodeIter<'s, P>> where 'a: 's;

    fn keys<'s>(&'s self) -> impl NodeIter<'s, &'s Self::K> where 'a: 's;

    //fn values<'s>(&'s self) -> impl NodeIter<'s, &'s Self::V> where 'a: 's;

    //fn pairs<'s>(&'s self) -> impl NodeIter<'s, (&'s Self::K, &'s Self::V)> where 'a: 's;

    fn children<'s>(&'s self) -> impl NodeIter<'s, P> where 'a: 's;

    fn children_mut<'s>(&'s mut self) -> impl NodeIterMut<'s, P> where 'a: 's;

    fn sibling_ptrs(&mut self) -> Option<(&mut P, &mut P)>;

    fn parent_ptr(&mut self) -> Option<&mut P>;

    fn self_ptr(&mut self) -> Option<&mut P>;

    fn remove_child(&mut self, k: &Self::K, child_idx: usize); //if node has no keys afterward it should be removed.

    ///the max degree of the node type, how many children it can possibly have.
    fn degree() -> usize;

    ///is this node at capacity (no room for another child)? default: `children().len() >=
    /// degree()`. valid only on INTERNAL nodes (`children()` reads child vaddrs; terminal nodes
    /// hold `SlicePtr`s there). the driver pre-checks this to decide insert-vs-split (it can't
    /// `Node::insert` on a full internal node — that would shrink it in place and orphan the
    /// right-half children; `split_internal` clones instead).
    fn is_full(&self) -> bool {
        let mut it = self.children();
        let n = it.len();
        let _ = it; //drop the borrow
        n >= Self::degree()
    }

    ///route `k` to a child index at this node, or None if it can't (no children, or
    ///the key doesn't map to a slot). Node-level routing ONLY — does NOT decide when
    ///to stop descending. A b+tree internal node with children infallibly routes;
    ///the probe/walker combine this with their own height awareness (a consumer
    ///field, NOT on this trait) to stop at terminal nodes so they don't descend
    ///into leaves. Nodes aren't forced to store height — that's a consumer-impl
    ///detail for the tree/walker/probe.
    fn try_route<'s>(&'s self, k: &Self::K) -> Option<usize> where 'a: 's;

    ///usize->P slot write: set the child ptr at child_idx. the only setter the
    ///block-tier needs; parent/sibling rewiring uses the NodeIter<&mut P> accessors.
    fn update_child(&mut self, child_idx: usize, new_p: P);

    ///drop the child ptr at child_idx (for remove).
    fn clear_child(&mut self, child_idx: usize);

    ///logical split: self keeps the left half, returns (right half, separator).
    ///`mid = child_count >> 1`; separator = the boundary key (keys[mid-1]) promoted to
    ///the parent. PURELY logical — no phys placement, no pointer fixup. INode impl == `split_off(mid)`.
    fn split(&mut self) -> (Self, Self::K);

    ///non-mutating right-half extraction: returns (right half, separator) without altering
    ///self, so the node stays full & wired (its right-half children reachable) until the
    ///right half is placed & wired — only then `truncate_to_left_half` shrinks it. default:
    ///clone self and split the clone.
    fn right_half(&self) -> (Self, Self::K)
    where Self: Clone {
        let mut clone = self.clone();
        clone.split()
    }

    ///shrink self to the left half in place (drop the right half). default: split and discard
    ///the returned right node. call AFTER the right half has been placed & wired.
    fn truncate_to_left_half(&mut self) {
        let _ = self.split();
    }

    ///insert (k, payload) in order; if full, split first then route it into the owning half.
    ///uniform over leaves (`Payload::Value`) and internals (`Payload::Child`): the SAME fn
    ///inserts a bucket into a terminal node and a child ptr into an internal during split
    ///propagation. both halves have room post-split (DEGREE>=3), so at most one split per call.
    ///returns the overflow for the walker to propagate up the parent stack, or None if it fit.
    ///INode impl == `insert_bucket` + `split_off` when full, mapping the `Payload` arm to `PtrUnion`.
    fn insert(&mut self, k: Self::K, payload: Payload<Self::V, P>) -> Option<Overflow<Self, Self::K>>;
}

///node may store its elements sparse, next/prev isnt necessarily position +- 1;
///`'a` is the iterator's scope (the borrow of the node it reads).
pub trait NodeIterBase<'a, T> {

    fn position(&self) -> usize;

    fn len(&self) -> usize; //number of elements, not necessarily max position.

    fn cap(&self) -> usize; //max in bounds position + 1.

    fn prev(&mut self);

    fn next(&mut self);

    fn seek(&mut self, p: usize);
}

pub trait NodeIter<'a, T>: NodeIterBase<'a, T> {

    fn current(&self) -> T;
}

///mut cursor. Decoupled from `NodeIter` (no `current(&self)`) because handing out
///`&'a mut` from a shared `&self` is unsound; `current_mut` reborrows per call.
pub trait NodeIterMut<'a, T>: NodeIterBase<'a, T> {

    fn current_mut(&mut self) -> &mut T;
}

///current vaddr only — no lineage. Every tree-tier reader needs at least this; the mut
///walker adds the ancestor stack + sibling nav via `TreeNav`. Holds no block borrow, so
///the shared `TreeProbe` and the mut `TreeWalkerMut` can both carry it without a
///shared-vs-mut lifetime clash.
pub trait TreePos<P: BlockIndex> {
    fn position(&self) -> P; //current node vaddr
    fn set_position(&mut self, p: P);
    ///current node's height (P::MIN = terminal). the terminal detector for walks that
    ///must not read a terminal node's `children()` (those hold SlicePtrs, not child vaddrs).
    fn height(&self) -> P;
    ///set the height counter. used by `insert_2` to reset to the root height before
    ///re-descending from the (pinned, stable) root between placements.
    fn set_height(&mut self, h: P);
}

///walker navigation: the ancestor stack + sibling/cousin stepping, on top of `TreePos`.
///Only the mut walker needs this — insert/remove fixup reads the ancestor stack
///(`parent`) and steps siblings (`next`/`prev`). The read probe is descend-to-leaf with
///no lineage, so it stops at `TreePos`.
pub trait TreeNav<P: BlockIndex>: TreePos<P> {

    ///take the ancestor stack: (parent vaddr, child_idx taken).
    fn pop(&mut self) -> Option<(P, usize)>;

    ///push (parent vaddr, child_idx) onto the ancestor stack.
    fn push(&mut self, parent: P, child_idx: usize);

    ///view the top of the ancestor stack (parent vaddr, child_idx).
    fn parent(&self) -> Option<(P, usize)>;

    fn ascend(&mut self); //goto parent

    fn next(&mut self); //go to next node in the defined ordering

    fn prev(&mut self); //go to prev node in the defined ordering

    fn right(&mut self); //go to prev sibling/cousin, skipping parent.

    fn left(&mut self); //go to next sibling/cousin, skipping parent
}

/// type that the block stores, ptrs type, address translator, ordering, store.
/// K/V come from T (Node); no separate K/V params. `'a` is the outlives bound only.
/// `Inner` is the raw block type the tree wraps; the walker/probe operate on `Inner`
/// directly (not on the tree), so `TreeBlock` need not — and does not — impl
/// `BlockMutTrait`. Block-mut is hidden behind the walker's invariant-upholding methods.
/// `Tree` is standalone (not a `BlockTrait` subtrait): `T`/`P`/`K`/`V`/`O`/`Inner` are its
/// own associated types. The walker/probe are parameterized over `Inner` + `O` (not over
/// `B: Tree`), so they reach `T`/`P` as direct `Inner::T`/`Inner::P` projections — no
/// equality tie between `Tree::T` and `<Inner as BlockTrait>::T` to deduce (which
/// rust-analyzer's solver can't). `A`/`S` aren't needed at the tree tier; recover them as
/// `Inner::A`/`Inner::S` when (if) the arena tier wants them.
pub trait Tree<'a>: Sized
where <Self::Inner as BlockTrait<'a>>::T: Node<'a, <Self::Inner as BlockTrait<'a>>::P, Self::O>
{
    type Inner: BlockMutTrait<'a> + 'a;
    type O: Ordering;
    ///tree-specific context (arbitrary — the UBTree stores height here). Seeded into
    ///walker/probe via `meta`; bumped via `meta_mut` on a root split.
    type Meta;

    fn root(&self) -> TreeP<'a, Self>;
    fn root_mut(&mut self) -> &mut TreeP<'a, Self>;
    fn meta(&self) -> &Self::Meta;
    fn meta_mut(&mut self) -> &mut Self::Meta;
    fn inner(&self) -> &Self::Inner;
    fn inner_mut(&mut self) -> &mut Self::Inner;

    ///borrow the tree for `'b`; caller picks the walker type. The walker sits above
    ///`Tree` — created from `&'b mut Self` (it holds the whole tree, so it can update
    ///root/meta on a split).
    fn walk_to<'b, W>(&'b mut self, k: &TreeK<'a, Self>) -> W
    where W: Walker<'b, 'a, Self>, 'a: 'b;

    fn probe<'b, Pr>(&'b self, k: &TreeK<'a, Self>) -> Pr
    where Pr: Probe<'b, 'a, Self>, 'a: 'b;
}

///shared read-only routing surface over a `Tree`: pick a child, read the current node,
///read a child vaddr. Param'd over the tree type `T` (the single witness — everything
///projects `T::P`/`T::Inner`/`T::T`, so probe/walker agree by construction, no equality
///bounds). `block()` is tied to `&self` (lending); both `Probe` and `Walker` extend this.
pub trait TreeRoute<'b, 'a, T: Tree<'a>>: TreePos<TreeP<'a, T>>
where 'a: 'b
{
    ///consumer-known: how to pick the child index for a key at the current node.
    fn try_route(&self, k: &TreeK<'a, T>) -> Option<usize>;

    ///the block, borrowed for the call (tied to `&self`, NOT `'b`). `&mut` isn't `Copy`,
    ///so no body returns `&'b` from `&self` — only a reborrow for `&self`'s scope.
    fn block(&self) -> &T::Inner;

    ///`'s` is the call's self-borrow; `where 'a: 's` matches `get`'s bound (the trait
    ///knows `'a: 'b` but not `'b: 's`). Required because `block()` is `&self`-tied.
    fn current<'s>(&'s self) -> &'s TreeT<'a, T>
    where 'a: 's {
        self.block().get(self.position())
    }

    ///child vaddr at child_idx of the current node.
    fn child_at<'s>(&'s self, child_idx: usize) -> TreeP<'a, T>
    where 'a: 's {
        let mut it = self.block().get(self.position()).children();
        it.seek(child_idx);
        it.current()
    }

    ///Subtree extremity of `child_v`: descend taking the last child (`right`) or
    /// child 0 (`!right`) until terminal.
    ///
    /// `child_v` — vaddr of a direct child of the current node.
    /// `child_height` — height of `child_v` (one below the current node).
    /// `right` — true → rightmost descendant, false → leftmost.
    /// Pure read: does not move `self.position` or touch the ancestor stack.
    /// Stops at `MIN` so it never reads a terminal node's `children()` (SlicePtrs).
    fn extremity_at(&self, child_v: TreeP<'a, T>, child_height: TreeP<'a, T>, right: bool) -> TreeP<'a, T> {
        let mut cur_v = child_v;
        let mut cur_h = child_height;
        while cur_h > <TreeP<'a, T> as Num>::MIN {
            //last child going right, child 0 going left
            let child_count = self.block().get(cur_v).children().len();
            let pick = if right { child_count.saturating_sub(1) } else { 0 };
            let mut child_iter = self.block().get(cur_v).children();
            child_iter.seek(pick);
            cur_v = child_iter.current();
            cur_h = cur_h.wrapping_sub(<TreeP<'a, T> as Num>::ONE);
        }
        cur_v
    }
    ///Leftmost descendant of `child_v` (a direct child of the current node):
    /// descend child 0 to terminal, return its vaddr.
    fn leftmost_desc(&self, child_v: TreeP<'a, T>) -> TreeP<'a, T> {
        self.extremity_at(child_v, self.height().wrapping_sub(<TreeP<'a, T> as Num>::ONE), false)
    }
    ///Rightmost descendant of `child_v` (a direct child of the current node):
    /// descend child k-1 to terminal, return its vaddr.
    fn rightmost_desc(&self, child_v: TreeP<'a, T>) -> TreeP<'a, T> {
        self.extremity_at(child_v, self.height().wrapping_sub(<TreeP<'a, T> as Num>::ONE), true)
    }
}

///read-only probe over a `Tree`: owns a `&'b T` borrow. Descend-to-leaf — no lineage
///(extends `TreeRoute`), so it adds only its shared-ref constructor + a no-push `descend`.
///Sits above `Tree`; created from `&'b T`.
pub trait Probe<'b, 'a, T: Tree<'a>>: Sized + TreeRoute<'b, 'a, T>
where 'a: 'b
{
    ///consumer constructs their probe from the tree (seeds root/meta from it).
    fn new(tree: &'b T) -> Self;

    ///step into child_idx. No lineage — probe is descend-to-leaf.
    fn descend<'s>(&'s mut self, child_idx: usize)
    where 'a: 's {
        self.set_position(self.child_at(child_idx));
    }
}

///owning-mut walker over a `Tree`: owns a `&'b mut T` borrow + lineage nav (`TreeNav`).
///Extends `TreeRoute` (shared read surface) and adds the mut surface + a `descend` that
///records lineage. Sits above `Tree`; created from `&'b mut T`. `insert_child`/
///`insert_as_parent`/`remove` do arena placement + moved-ptr fixup ONLY — they do NOT
///wire the new P into any node (node insert semantics are consumer-specific and
///unknowable at this tier). Raw block-mut is reachable only through these defaults.
pub trait Walker<'b, 'a, T: Tree<'a>>:
    Sized + TreeRoute<'b, 'a, T> + TreeNav<TreeP<'a, T>>
where 'a: 'b
{
    ///consumer constructs their walker from the tree (seeds root/meta from it).
    fn new(tree: &'b mut T) -> Self;

    ///the block, mut-borrowed for the call (tied to `&mut self`, not `'b`). Lending.
    fn block_mut(&mut self) -> &mut T::Inner;

    ///cached root vaddr (the pin).
    fn root(&self) -> TreeP<'a, T>;

    ///set the tree's root vaddr (used by `insert_root` to track the old root as it
    ///slides, then to point at the new root after the swap).
    fn set_root(&mut self, root: TreeP<'a, T>);

    ///bump the tree's height meta (the root's level) — called when `split` promotes a
    ///new root. impl-specific (lives on the tree's `Meta`, e.g. `InodeWalker`'s `TreeBlock`).
    fn bump_height(&mut self);

    ///the tree's current root height (the `Meta`). read fresh — a root split bumps it.
    fn root_height(&self) -> TreeP<'a, T>;

    fn current_mut<'s>(&'s mut self) -> &'s mut TreeT<'a, T>
    where 'a: 's {
        let pos = self.position();
        self.block_mut().get_mut(pos)
    }

    ///step into child_idx, recording lineage (overrides the probe's no-push descend).
    fn descend<'s>(&'s mut self, child_idx: usize)
    where 'a: 's {
        let cur = self.position();
        let child = self.child_at(child_idx);
        self.push(cur, child_idx);
        self.set_position(child);
    }

    ///Insert a new node in the arena as a child of current.
    ///Arena-places it near the anchor `insert_position` picks, fixes moved nodes' inbound ptrs.
    ///Returns its vaddr; does NOT wire it into current (consumer's job).
    ///
    /// `child_idx` — slot in current's children where the new child will land; used by insert_position to pick the anchor.
    /// `node` — the new node value to place; its children (if any) are adopted as a subtree.
    fn insert_child<'s>(&'s mut self, child_idx: usize, node: TreeT<'a, T>) -> Result<TreeP<'a, T>, TreeT<'a, T>>
    where 'a: 's {
        let parent_v = self.position();
        let relation = self.block().get(parent_v).insert_position(parent_v, child_idx);
        let (anchor_v, after) = match relation {
            RelTo::Before(p) => (p, false),
            RelTo::After(p) => (p, true),
        };
        //subtree-aware: if the new child is internal (height > MIN) and adopts a subtree,
        //anchor at that subtree's extremity (rightmost for After, leftmost for Before) so
        //the node lands adjacent to its own subtree — physical order stays == in-order
        //walk order (invariant 3). a terminal new child (height MIN) keeps the immediate
        //anchor (its extremity is itself). the new child sits at height self.height()-1;
        //its adopted children at self.height()-2.
        let child_height = self.height().wrapping_sub(<TreeP<'a, T> as Num>::ONE);
        let anchor_v = if child_height > <TreeP<'a, T> as Num>::MIN {
            let child_count = node.children().len();
            if child_count > 0 {
                let extreme_idx = if after { child_count - 1 } else { 0 };
                let mut child_iter = node.children();
                child_iter.seek(extreme_idx);
                let extreme_child_v = child_iter.current();
                self.extremity_at(extreme_child_v, child_height.wrapping_sub(<TreeP<'a, T> as Num>::ONE), after)
            } else {
                anchor_v
            }
        } else {
            anchor_v
        };
        //the root is pinned
        let pin = Some(self.root());
        let slide = match self.block_mut().find_slot(anchor_v, after, pin) {
            Some(slide) => slide,
            None => return Err(node),
        };

        if slide.from != slide.to {
            eprintln!("[fixup] SLIDE from={} to={} dir={} anchor={:?} anchor_p0={} parent_v={:?}", slide.from, slide.to, after, anchor_v, self.block().v2p(anchor_v), parent_v);
            //fixup: rewire inbound ptrs of moved nodes BEFORE slide_none, via the
            //ancestor stack (parent vaddr + child_idx) + update_child.

            //move cursor to anchor — find the child slot holding `anchor_v` rather
            //than assuming it's at child_idx (that's the new child's slot, not the anchor's).
            let anchor_p0 = self.block().v2p(anchor_v);
            if anchor_v != parent_v {
                let anchor_child_idx = {
                    let mut child_iter = self.current().children();
                    child_iter.seek(0);
                    let mut found = None;
                    for _ in 0..child_iter.len() {
                        if child_iter.current() == anchor_v { found = Some(child_iter.position()); break; }
                        child_iter.next();
                    }
                    found.expect("anchor not found among current node's children")
                };
                self.descend(anchor_child_idx);
            }
            let moved_count = slide.from.abs_diff(slide.to);
            let anchor_skip = if (slide.from > slide.to) == after { 0 } else { 1 }; //0 if anchor is not moving
            let moving_right = slide.from > anchor_p0; //right = true
            let phys_step: usize = if slide.from < slide.to { usize::MAX } else { 1 };

            let mut moved_phys = anchor_p0.wrapping_add(phys_step);
            if anchor_skip == 0 {
                if moving_right { self.next() } else { self.prev() }
                moved_phys = moved_phys.wrapping_add(phys_step);
            }
            for _ in anchor_skip..moved_count {
                let new_v = self.block().p2v(moved_phys);
                let (moved_parent_v, child_slot) = self.parent().unwrap();
                eprintln!("[fixup] cur_pos={:?} parent=({:?},{}) anchor_p={} new_v={:?} go_next={}", self.position(), moved_parent_v, child_slot, moved_phys, new_v, moving_right);
                self.block_mut().get_mut(moved_parent_v).update_child(child_slot, new_v);
                if moving_right { self.next() } else { self.prev() }
                moved_phys = moved_phys.wrapping_add(phys_step);
            }
        }

        let open_slot = self.block_mut().slide_none(slide, pin);
        let new_v = self.block_mut().insert(node, open_slot);
        Ok(new_v)
    }

    ///Promote a new root over the current root.
    ///The new root takes the root vaddr (tree.root unchanged under promotion); old root becomes child 0 and moves to its new slot.
    ///Returns the old root's new vaddr for the caller to wire as child 0.
    ///
    /// `old_root` — the current root's vaddr; becomes child 0 of the new root, free to slide (no pin).
    /// `new_root_node` — the new root node to place at the root vaddr.
    fn insert_root<'s>(
        &'s mut self,
        old_root: TreeP<'a, T>,
        new_root_node: TreeT<'a, T>,
    ) -> Result<TreeP<'a, T>, TreeT<'a, T>>
    where 'a: 's {
        //no pin: the old root is free to slide. tree.root stays to receive the new root,
        //so the old root's new vaddr is tracked locally (NOT via set_root).
        let relation = new_root_node.insert_position(old_root, 0);
        let (anchor_v, after) = match relation {
            RelTo::Before(p) => (p, false),
            RelTo::After(p) => (p, true),
        };
        let slide = match self.block_mut().find_slot(anchor_v, after, None) {
            Some(slide) => slide,
            None => return Err(new_root_node),
        };
        let mut old_root_new_v = old_root;
        if slide.from != slide.to {
            let anchor_p0 = self.block().v2p(anchor_v);
            let moved_count = slide.from.abs_diff(slide.to);
            let anchor_skip = if (slide.from > slide.to) == after { 0 } else { 1 };
            let moving_right = slide.from > anchor_p0;
            let phys_step: usize = if slide.from < slide.to { usize::MAX } else { 1 };

            let mut moved_phys = anchor_p0.wrapping_add(phys_step);
            if anchor_skip == 1 {
                //the anchor (old root) shifts one slot toward the freed None; track its new
                //vaddr locally. tree.root is NOT moved — it stays to receive the new root.
                old_root_new_v = self.block().p2v(moved_phys);
                if moving_right { self.next() } else { self.prev() }
                moved_phys = moved_phys.wrapping_add(phys_step);
            } else {
                if moving_right { self.next() } else { self.prev() }
                moved_phys = moved_phys.wrapping_add(phys_step);
            }
            for _ in anchor_skip..moved_count {
                let new_v = self.block().p2v(moved_phys);
                let (moved_parent_v, child_slot) = self.parent().unwrap();
                self.block_mut().get_mut(moved_parent_v).update_child(child_slot, new_v);
                if moving_right { self.next() } else { self.prev() }
                moved_phys = moved_phys.wrapping_add(phys_step);
            }
        }

        let root_v = self.root();
        let open_slot = self.block_mut().slide_none(slide, None);
        let open_v = self.block_mut().p2v(open_slot.0);
        if open_v == root_v {
            //the slide freed the root slot → new root takes it directly; tree.root unchanged.
            let _ = self.block_mut().insert(new_root_node, open_slot);
        } else {
            //new root placed off the root slot → swap it in; old root lands at open_v.
            let new_v = self.block_mut().insert(new_root_node, open_slot);
            self.block_mut().swap(new_v, root_v);
            old_root_new_v = open_v;
        }
        Ok(old_root_new_v)
    }

    ///Remove the current node from the arena. It must have no children.
    ///
    /// Returns the removed node, or `None` if the current node is the root (no
    /// parent to clear the child slot from — caller handles that).
    fn remove<'s>(&'s mut self) -> Option<TreeT<'a, T>>
    where 'a: 's {
        let cur_v = self.position();
        //no parent => current is the root; nothing to clear
        let (parent_v, child_idx) = self.parent()?;
        let removed_node = self.block_mut().remove(cur_v);
        self.block_mut().get_mut(parent_v).clear_child(child_idx);
        Some(removed_node)
    }

    ///Relocate an EXISTING wired node to its in-order gap after its anchor child shifted
    /// (a left-half child split inserted a sibling that became the new `child[half-1]`).
    /// `self` must be positioned at the new anchor — `rightmost_desc(node.children[half-1])`
    /// — with a valid ancestor stack; pin = root.
    ///
    /// `node_v` — handle to the node to relocate; updated in place to its final vaddr.
    ///   The node is treated as floating during the slide (its inbound pointer is rewritten
    ///   by the CALLER, not by `fixup_moved_run`'s parent-walk), so the handle captures its
    ///   post-slide vaddr.
    /// `floating` — other unwired handles this hop's slide might move (rewritten in place).
    ///
    /// Order: find_slot opens a gap at the anchor; fixup (run BEFORE slide_none) pre-writes
    /// post-slide vaddrs into moved nodes' parent pointers and rewrites the floating handle;
    /// slide_none actually shifts; swap_open then moves the node the rest of the way to the
    /// opened gap (the slide may have shifted it by one if it was in the run).
    fn hop_to_median<'s>(
        &'s mut self,
        node_v: &mut TreeP<'a, T>,
        floating: &mut [TreeP<'a, T>],
    ) where 'a: 's {
        let anchor_v = self.position();
        let root = self.root();
        //the node itself is floating (caller rewrites its inbound pointer); merge any
        //caller-passed handles so fixup rewrites them all in one pass.
        let mut floating_all: Vec<TreeP<'a, T>> = Vec::with_capacity(1 + floating.len());
        floating_all.push(*node_v);
        floating_all.extend(floating.iter().copied());
        let slide = match self.block_mut().find_slot(anchor_v, true, Some(root)) {
            Some(s) => s,
            None => panic!("hop_to_median: arena full"),
        };
        if slide.from != slide.to {
            self.fixup_moved_run(slide, &mut floating_all);
            //fixup rewrote floating_all[0] if the node was in the slide run.
            *node_v = floating_all[0];
        }
        let open = self.block_mut().slide_none(slide, Some(root));
        //the node is now at its post-slide phys (= v2p(*node_v)); swap it into the opened gap.
        let (_freed, new_v) = self.block_mut().swap_open(*node_v, open);
        *node_v = new_v;
        //write back any caller-passed handles that fixup may have updated.
        let caller_floating = &mut floating[..];
        for (f, fa) in caller_floating.iter_mut().zip(floating_all.iter().skip(1)) {
            *f = *fa;
        }
        //caller updates the node's inbound pointer: grandparent.children[idx] = *node_v.
    }

    /// Rewrite each moved node's inbound child pointer after a slide.
    /// Floating nodes (in `floating`) have no wired parent — their handle is updated instead.
    /// The run is contiguous in-order; next()/prev() + the ancestor stack enumerate it.
    ///
    /// `slide` — NoneSlide of the run; `from` is the gap (a None), `to` its destination.
    /// `floating` — vaddrs of placed-but-unwired nodes whose handles get rewritten in place.
    fn fixup_moved_run<'s>(
        &'s mut self,
        slide: crate::store::NoneSlide,
        floating: &mut [TreeP<'a, T>],
    ) where 'a: 's {
        if slide.from == slide.to { return; }
        //caller positioned `self` at the anchor; derive anchor_p0 from it.
        let anchor_p0 = self.block().v2p(self.position());
        let moved_count = slide.from.abs_diff(slide.to);
        let moving_right = slide.from > anchor_p0;       //right = true
        let phys_step: usize = if slide.from < slide.to { usize::MAX } else { 1 };
        //anchor is in the moved run iff it lies strictly between from and to (the None
        //at `from` is a gap, not a node). `anchor_moves` is false when the anchor is
        //stationary, true when it shifts with the run.
        let anchor_moves = if slide.from < slide.to {
            slide.from < anchor_p0 && anchor_p0 <= slide.to
        } else {
            slide.to <= anchor_p0 && anchor_p0 < slide.from
        };
        //node now at moved_phys was previously at moved_phys ∓ phys_step (it shifted one
        //slot toward `from`); that old phys yields the old vaddr the parent/handle still holds.
        let mut moved_phys = anchor_p0.wrapping_add(phys_step);
        if !anchor_moves {
            //anchor stationary: step past it before entering the moved run.
            if moving_right { self.next() } else { self.prev() }
            moved_phys = moved_phys.wrapping_add(phys_step);
        }
        for _ in 0..moved_count {
            let new_v = self.block().p2v(moved_phys);
            let old_v = self.block().p2v(moved_phys.wrapping_sub(phys_step));
            //floating handle → rewrite in place; else rewrite the parent's child pointer.
            if let Some(h) = floating.iter_mut().find(|h| **h == old_v) {
                *h = new_v;
            } else {
                let (parent_v, child_slot) = self.parent().expect("moved wired node has no parent");
                self.block_mut().get_mut(parent_v).update_child(child_slot, new_v);
            }
            if moving_right { self.next() } else { self.prev() }
            moved_phys = moved_phys.wrapping_add(phys_step);
        }
    }

    /// `self` must be positioned at the anchor with a valid ancestor stack.
    /// Place one new unparented node at `target_gap = phys(anchor)+1` in direction `after`.
    /// Err(node) if the arena is full.
    ///
    /// `node` — the new unparented node to place.
    /// `after` — direction bool (true = insert AFTER anchor = rightward).
    /// `floating` — vaddrs of placed-but-unwired nodes this slide might move (handles updated in place).
    /// `pin` — a vaddr clamping the slide so it never crosses it.
    fn place_one<'s>(
        &'s mut self,
        node: TreeT<'a, T>,
        after: bool,
        floating: &mut [TreeP<'a, T>],
        pin: TreeP<'a, T>,
    ) -> Result<TreeP<'a, T>, TreeT<'a, T>>
    where 'a: 's {
        let anchor_v = self.position();
        let slide = match self.block_mut().find_slot(anchor_v, after, Some(pin)) {
            Some(slide) => slide,
            None => return Err(node),
        };
        if slide.from != slide.to {
            self.fixup_moved_run(slide, floating);
        }
        let slot = self.block_mut().slide_none(slide, Some(pin));
        Ok(self.block_mut().insert(node, slot))
    }

    /// Position `self` at `rightmost_desc(self.children[child_idx])` by descending.
    /// Pushes the ancestor stack each level; `self` starts at the parent.
    /// NOT `TreeRoute::rightmost_desc` (read-only, no stack) — this builds the lineage fixup_moved_run needs.
    ///
    /// `child_idx` — index of the child whose rightmost descendant is the target.
    fn descend_to_rightmost_desc<'s>(&'s mut self, child_idx: usize)
    where 'a: 's {
        self.descend(child_idx);
        while self.height() > <TreeP<'a, T> as Num>::MIN {
            //descend into the last child each level until terminal
            let child_count = self.block().get(self.position()).children().len();
            self.descend(child_count.saturating_sub(1));
        }
    }

    /// Reset `self` to root (position + height + empty stack), descend `path` (child indices
    /// root→Y) to Y, then `descend_to_rightmost_desc(child_idx)` to the anchor terminal.
    /// The root is pinned (inv 4) so it never moves; re-descending from it reaches Y's
    /// CURRENT position even if Y moved in a prior placement's slide.
    ///
    /// `root_v` — vaddr of the pinned root to re-descend from.
    /// `root_height` — height of the root (tree height).
    /// `path` — child indices root→Y to re-descend to Y.
    /// `child_idx` — index of the child whose rightmost descendant is the anchor.
    fn reposition_to_anchor<'s>(
        &'s mut self,
        root_v: TreeP<'a, T>,
        root_height: TreeP<'a, T>,
        path: &[usize],
        child_idx: usize,
    ) where 'a: 's {
        self.set_position(root_v);
        self.set_height(root_height);
        //clear the ancestor stack
        while self.pop().is_some() {}
        //re-descend root→Y
        for &ci in path {
            self.descend(ci);
        }
        self.descend_to_rightmost_desc(child_idx);
    }

    /// Re-derive a node's position by routing `key` from the root, stopping at `target_height`.
    /// Returns (node_v, parent_v_opt, child_idx) where child_idx is the node's index in its
    /// parent (parent is None iff the node is the root). Reads `root_height` fresh — a root
    /// split may have bumped it. Used to re-derive after splits/hops move things.
    fn route_to_height<'s>(&'s mut self, key: &TreeK<'a, T>, target_height: TreeP<'a, T>)
        -> (TreeP<'a, T>, Option<TreeP<'a, T>>, usize)
    where 'a: 's {
        let rh = self.root_height();
        self.set_position(self.root());
        self.set_height(rh);
        while self.pop().is_some() {}
        let mut parent_v = self.root();
        let mut child_idx = 0;
        while self.height() > target_height {
            let ci = self.try_route(key).expect("route_to_height: key does not route to target");
            parent_v = self.position();
            child_idx = ci;
            self.descend(ci);
        }
        let parent = if target_height == rh { None } else { Some(parent_v) };
        (self.position(), parent, child_idx)
    }

    /// After placing a split's right half, its child vaddrs (copied from the source node
    /// BEFORE the placement's slide) may be stale: a child that moved during the slide had its
    /// inbound pointer updated in the SOURCE node (wired, on the slide's ancestor stack), not
    /// in the right half (floating). Re-copy the right half's children from the source node's
    /// (still-full, fixup-current) children[mid..n] before the source is truncated. Only valid
    /// for internal nodes (leaves hold SlicePtrs, not child vaddrs).
    fn refresh_right_half_children<'s>(
        &'s mut self,
        right_v: TreeP<'a, T>,
        src_v: TreeP<'a, T>,
        mid: usize,
    ) where 'a: 's {
        let n = self.block().get(src_v).children().len();
        let mut kids: Vec<TreeP<'a, T>> = Vec::with_capacity(n - mid);
        {
            let mut it = self.block().get(src_v).children();
            it.seek(mid);
            for _ in mid..n {
                kids.push(it.current());
                it.next();
            }
        }
        let right = self.block_mut().get_mut(right_v);
        for (i, &cv) in kids.iter().enumerate() {
            right.update_child(i, cv);
        }
    }

    /// Split the root (full, no parent): place the right half after `rightmost_desc(root)`,
    /// shrink the root to the left half in place (keeps the root vaddr), then `promote_new_root`
    /// over [left half, right half] — the new root takes the fixed root vaddr (inv 2; the root
    /// is pinned at MIDPOINT, inv 4) and the old root (left half) is re-placed just before it.
    /// `next`/`prev` tolerate the root's 2-child in-order anomaly (root exemption). Pin the
    /// old root throughout so it doesn't move.
    fn split_root<'s>(&'s mut self, _key: &TreeK<'a, T>, _node_height: TreeP<'a, T>)
    where 'a: 's {
        let half = <TreeT<'a, T> as Node<'a, TreeP<'a, T>, <T as Tree<'a>>::O>>::degree() / 2;
        let root_v = self.root();
        let (right_node, sep) = self.block().get(root_v).right_half();
        // position at rightmost_desc(root) — the right half's in-order gap.
        self.set_position(root_v);
        self.set_height(self.root_height());
        while self.pop().is_some() {}
        if self.height() > <TreeP<'a, T> as Num>::MIN {
            let last = self.block().get(root_v).children().len() - 1;
            self.descend_to_rightmost_desc(last);
        }
        let right_v = self.place_one(right_node, true, &mut [], root_v)
            .unwrap_or_else(|_| panic!("split_root: arena full placing right half"));
        if self.root_height() > <TreeP<'a, T> as Num>::MIN {
            self.refresh_right_half_children(right_v, root_v, half);
        }
        self.block_mut().get_mut(root_v).truncate_to_left_half();
        self.promote_new_root(sep, right_v);
    }

    /// Promote a new internal root over [left half, right_v] at the fixed root vaddr (inv 2).
    /// The old root (now the left half, at root_v after truncate) is removed, the new root
    /// takes its slot (root vaddr stable — the root is pinned at MIDPOINT, inv 2/4), and the
    /// left half is re-placed just before the new root. Pin the root so it never moves.
    fn promote_new_root<'s>(&'s mut self, separator: TreeK<'a, T>, right_v: TreeP<'a, T>)
    where 'a: 's {
        let root = self.root();
        let left_node = self.block_mut().remove(root);
        let mut new_root_node: TreeT<'a, T> = Default::default();
        let _ = new_root_node.insert(separator, Payload::Child(right_v));
        new_root_node.update_child(0, root);
        let phys = self.block().v2p(root);
        let new_root_v = self.block_mut().insert(new_root_node, crate::block::OpenSlot(phys));
        debug_assert_eq!(new_root_v, root, "promote_new_root: new root not at root vaddr");
        self.set_position(root);
        while self.pop().is_some() {}
        let left_v = self.place_one(left_node, false, &mut [], root)
            .unwrap_or_else(|_| panic!("promote_new_root: arena full"));
        self.block_mut().get_mut(root).update_child(0, left_v);
        self.bump_height();
    }

    /// Recursively split the node at `node_height` on the path for `key`, and any full ancestors
    /// up to the first non-full one (or the root). Top-down in effect: the topmost full ancestor
    /// splits first (deepest recursion), so each split's parent is roomy when it runs. The left
    /// half stays at the old slot (fixed `DEGREE/2` in-order); the right half is placed fresh
    /// after `rightmost_desc(node)` and wired into the (roomy) parent immediately. If the right
    /// half lands at index `< half` in the parent, the parent relocates left via `hop_to_median`.
    fn split<'s>(&'s mut self, key: &TreeK<'a, T>, node_height: TreeP<'a, T>)
    where 'a: 's {
        let half = <TreeT<'a, T> as Node<'a, TreeP<'a, T>, <T as Tree<'a>>::O>>::degree() / 2;
        let one = <TreeP<'a, T> as Num>::ONE;
        let parent_height = node_height.wrapping_add(one);
        // re-derive the node + parent; recurse on a full parent first (top-down).
        let (_, parent_opt, _) = self.route_to_height(key, node_height);
        let parent_v = match parent_opt {
            None => { self.split_root(key, node_height); return; }
            Some(p) => p,
        };
        if self.block().get(parent_v).is_full() {
            self.split(key, parent_height);
        }
        // re-derive after the (possible) recursive split.
        let (node_v, parent_opt, child_idx) = self.route_to_height(key, node_height);
        let mut parent_v = parent_opt.expect("split: node became the root after recursive split");
        // extract the right half (node stays full & wired until truncate).
        let (right_node, sep) = self.block().get(node_v).right_half();
        // place the right half as a new child at index child_idx+1 of the parent.
        self.set_position(parent_v);
        self.set_height(parent_height);
        while self.pop().is_some() {}
        let right_v = self.insert_child(child_idx + 1, right_node)
            .unwrap_or_else(|_| panic!("split: arena full placing right half"));
        // re-derive parent + node (insert_child's slide may have moved them).
        let (node_v_after, parent_opt, child_idx) = self.route_to_height(key, node_height);
        parent_v = parent_opt.expect("split: node became the root after placement");
        // the right half's children may have moved during placement; fixup updated the node's
        // children (wired), so re-copy them into the right half before truncating. (internal
        // node only — a leaf's right half holds SlicePtrs, not child vaddrs.)
        if node_height > <TreeP<'a, T> as Num>::MIN {
            self.refresh_right_half_children(right_v, node_v_after, half);
        }
        // wire (sep, right_v) into the parent (roomy → in-place insert_bucket, no overflow).
        let overflow = self.block_mut().get_mut(parent_v).insert(sep, Payload::Child(right_v));
        debug_assert!(overflow.is_none(), "split: parent overflowed (should be roomy)");
        // if the right half landed at index < half, the parent's anchor child shifted → hop it.
        if child_idx + 1 < half {
            // position at the parent's new anchor (rightmost_desc(parent.children[half-1])).
            self.set_position(parent_v);
            self.set_height(parent_height);
            while self.pop().is_some() {}
            self.descend_to_rightmost_desc(half - 1);
            self.hop_to_median(&mut parent_v, &mut []);
            // update the parent's inbound pointer in the grandparent (re-derive — the hop's
            // slide may have moved the grandparent; fixup kept its inbound current, but we read
            // its current vaddr to write the parent's new vaddr into the right record).
            let (grandparent_v, _, parent_idx) = self.route_to_height(key, parent_height.wrapping_add(one));
            self.block_mut().get_mut(grandparent_v).update_child(parent_idx, parent_v);
        }
        // shrink the node to the left half in place. re-derive node_v from the parent
        // (parent.children[child_idx] — fixup kept it current through any slide).
        self.set_position(parent_v);
        self.set_height(parent_height);
        while self.pop().is_some() {}
        let node_v = self.child_at(child_idx);
        self.block_mut().get_mut(node_v).truncate_to_left_half();
    }
}

///tree-tier wrapper over a raw block. Owns the inner block + the root vaddr. Does NOT
///impl `BlockMutTrait` — the block-mut surface is reachable only through the walker's
/// invariant-upholding defaults, never on `TreeBlock` itself. `inner` is private;
/// `walk_to`/`probe` (defined here, in the same module) touch it directly.
/// `T`/`P`/`A`/`S` are all derived from `Inner` (via `BlockMutTrait`/`BlockTrait`
/// associated types), so the only type inputs are `Inner` + the ordering `O`.
pub struct TreeBlock<'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    ///private: callers go through TreeBlock.
    inner: Inner,
    ///root vaddr. stored; rotation makes it underivable from first/last.
    root: Inner::P,
    ///tree height (= root height); the `Meta` walker/probe seed from.
    height: Inner::P,
    _p: PhantomData<(&'a Inner::T, O)>,
}

impl<'a, Inner, O> TreeBlock<'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    ///seed: empty inner block + `insert_root` of `root_node` at the anchor vaddr.
    ///`height` is the tree height (= root height); stored as the walker/probe `Meta`.
    pub(crate) fn new(root_node: Inner::T, height: Inner::P) -> Self {
        let mut inner = Inner::new();
        let root = inner.insert_root(root_node);
        Self { inner, root, height, _p: PhantomData }
    }
}

//BlockTrait read surface forwards through inner. (Read-only; mut is NOT forwarded.)
impl<'a, Inner, O> BlockTrait<'a> for TreeBlock<'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    type T = Inner::T;
    type P = Inner::P;
    type S = Inner::S;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b {
        self.inner.store()
    }

    fn translator<'b>(&'b self) -> &'b Translator<Self::P> {
        self.inner.translator()
    }
}

impl<'a, Inner, O> fmt::Debug for TreeBlock<'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a + fmt::Debug,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeBlock")
            .field("root", &self.root)
            .field("height", &self.height)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<'a, Inner, O> Tree<'a> for TreeBlock<'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
    Self : 'a,
{
    type Inner = Inner;
    type O = O;
    type Meta = Inner::P;

    fn root(&self) -> TreeP<'a, Self> { self.root }
    fn root_mut(&mut self) -> &mut TreeP<'a, Self> { &mut self.root }
    fn meta(&self) -> &Inner::P { &self.height }
    fn meta_mut(&mut self) -> &mut Inner::P { &mut self.height }
    fn inner(&self) -> &Inner { &self.inner }
    fn inner_mut(&mut self) -> &mut Inner { &mut self.inner }

    fn walk_to<'b, W>(&'b mut self, k : &TreeK<'a, Self>)  -> W
    where W : Walker<'b, 'a, Self>, 'a: 'b {
        let mut walker = W::new(self);
        while let Some(child_idx) = walker.try_route(k) {walker.descend(child_idx)}
        walker
    }

    fn probe<'b, Pr>(&'b self, k : &TreeK<'a, Self>) -> Pr
    where Pr : Probe<'b, 'a, Self>, 'a: 'b {
        let mut probe = Pr::new(self);
        while let Some(child_idx) = probe.try_route(k) {probe.descend(child_idx)}
        probe
    }
}

// ─── concrete walker ─────────────────────────────────────────────────────────
// value semantics (map/keys_slice/etc.) live on INode in lib.rs; this module
// supplies the concrete height-tracking walker over any `T: Tree` whose `Meta`
// is a `Num` (used as the height counter: seeded from `T::meta`, decremented on
// descend, `try_route` stops at `Meta::MIN`). The probe is in lib.rs.

///positioned cursor over a `&'a [T]` slice — backs `INode::keys`.
pub struct SliceNodeIter<'a, T> { pub(crate) slice: &'a [T], pub(crate) idx: usize }

impl<'a, T> NodeIterBase<'a, &'a T> for SliceNodeIter<'a, T> {
    fn position(&self) -> usize { self.idx }
    fn len(&self) -> usize { self.slice.len() }
    fn cap(&self) -> usize { self.slice.len() }
    fn prev(&mut self) { self.idx = self.idx.saturating_sub(1); }
    fn next(&mut self) { self.idx = (self.idx + 1).min(self.slice.len()); }
    fn seek(&mut self, p: usize) { self.idx = p.min(self.slice.len()); }
}

impl<'a, T> NodeIter<'a, &'a T> for SliceNodeIter<'a, T> {
    fn current(&self) -> &'a T { &self.slice[self.idx] }
}


///Route `key` to its child index via the node's keys (binary search over the
/// positioned `NodeIter`). `Ok(i)` → right child (i+1); `Err(i)` → child i.
///
/// `node` — the node whose keys are searched.
/// `key` — the lookup key.
/// `'n` is the node's `Node` outlives lifetime; `'s` is the ref borrow (`'s ⊆ 'n`).
/// Decoupled from `'s` because callers prove `Inner::T: Node<'a, ...>` for a fixed
/// `'a`, and the node ref is tied to `&self` (`⊆ 'b ⊆ 'a`), not `'a` — tying the
/// `Node` lifetime to the ref would force `'s = 'a` (i.e. `'b: 'a`), which is wrong.
pub fn route_idx<'s, 'n, N, P, O>(node: &'s N, key: &N::K) -> usize
where
    N: Node<'n, P, O>,
    'n: 's,
    N::K: Ord,
    P: BlockIndex,
    O: Ordering,
{
    let mut key_iter = node.keys();
    let key_count = key_iter.len();
    let mut lo = 0;
    let mut hi = key_count;
    while lo < hi {
        let mid = (lo + hi) / 2;
        key_iter.seek(mid);
        match key_iter.current().cmp(key) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => return mid + 1,
        }
    }
    lo
}





#[cfg(test)]
#[path = "tests/tree.rs"]
mod tests;