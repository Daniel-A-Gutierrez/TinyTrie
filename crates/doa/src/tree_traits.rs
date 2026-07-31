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
pub trait Node<'a, P: BlockIndex, O: Ordering>: Sized + 'a + OrderedNode<P, O> {
    type K: Sized + 'a;
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

    ///insert a new node in the arena as a child of current. Arena-places it near the
    ///anchor `insert_position` picks, fixes moved nodes' inbound ptrs, returns its vaddr.
    ///Does NOT wire the returned P into current — that's the consumer's job (node insert
    ///semantics are unknowable at this tier).
    fn insert_child<'s>(&'s mut self, child_idx: usize, node: TreeT<'a, T>) -> Result<TreeP<'a, T>, TreeT<'a, T>>
    where 'a: 's {
        let parent_v = self.position();
        let rel = self.block().get(parent_v).insert_position(parent_v, child_idx);
        let (anchor, dir) = match rel {
            RelTo::Before(p) => (p, false),
            RelTo::After(p) => (p, true),
        };
        //the root is pinned
        let pin = Some(self.root());
        let slide = match self.block_mut().find_slot(anchor, dir, pin) {
            Some(slide) => slide,
            None => return Err(node),
        };

        if slide.from != slide.to {
            eprintln!("[fixup] SLIDE from={} to={} dir={} anchor={:?} anchor_p0={} parent_v={:?}", slide.from, slide.to, dir, anchor, self.block().v2p(anchor), parent_v);
            //fixup: rewire inbound ptrs of moved nodes BEFORE slide_none, via the
            //ancestor stack (parent vaddr + child_idx) + update_child.

            //move cursor to anchor — find the child slot holding `anchor` rather
            //than assuming it's at child_idx (that's the new child's slot, not the anchor's).
            let anchor_p0 = self.block().v2p(anchor);
            if anchor != parent_v {
                let idx = {
                    let mut it = self.current().children();
                    it.seek(0);
                    let mut found = None;
                    for _ in 0..it.len() {
                        if it.current() == anchor { found = Some(it.position()); break; }
                        it.next();
                    }
                    found.expect("anchor not found among current node's children")
                };
                self.descend(idx);
            }
            let count = slide.from.abs_diff(slide.to);
            let skip = if (slide.from > slide.to) == dir { 0 } else { 1 }; //0 if anchor is not moving
            let go_next = slide.from > anchor_p0; //right = true
            let delta: usize = if slide.from < slide.to { usize::MAX } else { 1 };

            let mut anchor_p = anchor_p0.wrapping_add(delta);
            if skip == 0 {
                if go_next { self.next() } else { self.prev() }
                anchor_p = anchor_p.wrapping_add(delta);
            }
            for _ in skip..count {
                let new_v = self.block().p2v(anchor_p);
                let (vaddr, ci) = self.parent().unwrap();
                eprintln!("[fixup] cur_pos={:?} parent=({:?},{}) anchor_p={} new_v={:?} go_next={}", self.position(), vaddr, ci, anchor_p, new_v, go_next);
                self.block_mut().get_mut(vaddr).update_child(ci, new_v);
                if go_next { self.next() } else { self.prev() }
                anchor_p = anchor_p.wrapping_add(delta);
            }
        }

        let slot = self.block_mut().slide_none(slide, pin);
        let new_v = self.block_mut().insert(node, slot);
        Ok(new_v)
    }

    fn insert_root<'s>(
        &'s mut self,
        old_root: TreeP<'a, T>,
        new_root: TreeT<'a, T>,
    ) -> Result<TreeP<'a, T>, TreeT<'a, T>>
    where 'a: 's {
        //new root takes the root vaddr (tree.root unchanged — the root's traversal position is
        //invariant under promotion). old root becomes child 0 → moves to its new traversal
        //slot; tracked locally (NOT via set_root — tree.root stays to receive the new root) and
        //returned for the caller to wire as child 0. no pin: the old root is free to slide.
        let rel = new_root.insert_position(old_root, 0);
        let (anchor, dir) = match rel {
            RelTo::Before(p) => (p, false),
            RelTo::After(p) => (p, true),
        };
        let slide = match self.block_mut().find_slot(anchor, dir, None) {
            Some(slide) => slide,
            None => return Err(new_root),
        };
        let mut old_root_new_v = old_root;
        if slide.from != slide.to {
            let anchor_p0 = self.block().v2p(anchor);
            let count = slide.from.abs_diff(slide.to);
            let skip = if (slide.from > slide.to) == dir { 0 } else { 1 };
            let go_next = slide.from > anchor_p0;
            let delta: usize = if slide.from < slide.to { usize::MAX } else { 1 };

            let mut anchor_p = anchor_p0.wrapping_add(delta);
            if skip == 1 {
                //the anchor (old root) shifts one slot toward the freed None; track its new
                //vaddr locally. tree.root is NOT moved — it stays to receive the new root.
                old_root_new_v = self.block().p2v(anchor_p);
                if go_next { self.next() } else { self.prev() }
                anchor_p = anchor_p.wrapping_add(delta);
            } else {
                if go_next { self.next() } else { self.prev() }
                anchor_p = anchor_p.wrapping_add(delta);
            }
            for _ in skip..count {
                let new_v = self.block().p2v(anchor_p);
                let (vaddr, ci) = self.parent().unwrap();
                self.block_mut().get_mut(vaddr).update_child(ci, new_v);
                if go_next { self.next() } else { self.prev() }
                anchor_p = anchor_p.wrapping_add(delta);
            }
        }

        let root_v = self.root();
        let open = self.block_mut().slide_none(slide, None);
        let open_v = self.block_mut().p2v(open.0);
        if open_v == root_v {
            //the slide freed the root slot → new root takes it directly; tree.root unchanged.
            let _ = self.block_mut().insert(new_root, open);
        } else {
            //new root placed off the root slot → swap it in; old root lands at open_v.
            let new_v = self.block_mut().insert(new_root, open);
            self.block_mut().swap(new_v, root_v);
            old_root_new_v = open_v;
        }
        Ok(old_root_new_v)
    }

    ///remove current from the arena. It must not have children.
    fn remove<'s>(&'s mut self) -> Option<TreeT<'a, T>>
    where 'a: 's {
        let cur_v = self.position();
        //None => removing the root; caller handles (no parent to clear).
        let (parent_v, child_idx) = self.parent()?;
        let v = self.block_mut().remove(cur_v);
        self.block_mut().get_mut(parent_v).clear_child(child_idx);
        Some(v)
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


///route `k` to its child index via the node's keys (binary search over the
///positioned NodeIter). Ok(i) => right child (i+1); Err(i) => child i.
///`'n` is the node's `Node` outlives lifetime; `'s` is the ref borrow (`'s ⊆ 'n`).
///Decoupled from `'s` because callers prove `Inner::T: Node<'a, ...>` for a fixed
///`'a`, and the node ref is tied to `&self` (`⊆ 'b ⊆ 'a`), not `'a` — tying the
///`Node` lifetime to the ref would force `'s = 'a` (i.e. `'b: 'a`), which is wrong.
pub fn route_idx<'s, 'n, N, P, O>(node: &'s N, k: &N::K) -> usize
where
    N: Node<'n, P, O>,
    'n: 's,
    N::K: Ord,
    P: BlockIndex,
    O: Ordering,
{
    let mut it = node.keys();
    let n = it.len();
    let mut lo = 0;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        it.seek(mid);
        match it.current().cmp(k) {
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