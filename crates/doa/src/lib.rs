mod abstract_tree;
mod alloc_strat;
mod block;
mod index;
mod inline_leafblock;
mod leafblock;
mod store;
mod translator;
mod tree_traits;
use crate::leafblock::{PtrUnion, SlicePtr};
use block::*;
use index::*;
use crate::translator::{Translator, AddressTranslator};
use std::{cmp::Ordering::{Equal, Greater, Less},
          collections::VecDeque,
          marker::PhantomData,
          ops::Range};
use tree_traits::*;

pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
pub trait Ordering: 'static {}

///easiest to split, iteration OK
impl Ordering for InOrder {}
impl Ordering for PostOrder {}
impl Ordering for PreOrder {}

enum RelTo<T> {
    Before(T),
    After(T),
}
pub(crate) type BPtr = i32;
pub(crate) type IPtr = u32;
pub(crate) type LPtr = u16;

//fractal forest
struct FractalForest<K: Ord + Sized + Clone, V: Sized> {

    ///root is at trees[0]
    root:   BTree<K, BPtr>, //map key to a terminal block
    ltrees: Vec<BTree<K, V>>,
    len:    usize,
}




struct BTree<K: Sized + Ord + Clone, V: Sized> {

    // inodes : block::Block<INode<K, IPtr, LPtr>,IPtr,PreOrder,Pluripotent>, //require preorder and fixed root, and pluripotent
    leaves: leafblock::LeafBlock<K, V, LPtr>, //leafblock is random so it can guarantee capacity so long as inodes max size is 4096 (for u16, fanout 16)
    height: u32,
    next:   u32,
    prev:   u32,
}



impl<K, V> BTree<K, V>
where
    K: Sized + Ord + Clone,
    V: Sized,
{
    /*

    fn new() -> Self {}

    fn insert(&mut self, K , V ) {
        if self.height==0 {self.leaves.root_insert(K,V));}
        if self.len == MAX { panic }
        let iroot = self.inodes.root_node();

        //do tree traversal to get terminal node in inodes
        let terminal_inode = //stuff;
        let leaf = terminal_inode.map(K).terminal;
        let next = //stuff to get next ptr after leaf.

        //check that there's enough space between next and leaf
        //if not, scan for a open space using the inode cursor and leaves.distance() up to some max distance.
        //if that fails, grow&spread, guaranteeing there's space between leaf and next.
        self.leaves.insert_between(leaf,(K,V),next.ptr)
    }

    fn remove

    fn get

    fn leaves_iter

    fn range

    fn split
    */
}

/*
plan : 
impl Node + Ordered Node for Inode 
create concrete walker type thats height aware and impl TreeWalkerMut<Ordering, RawBlockType> on it
ditto for probe
tree nav + trie pos on both?
Then we should get a TreeBlock<Inner, O> that we can get the walker and probe from.
Those should be capable of insert/lookup/remove

current success criteria : store PtrUnion<Iptr,Lptr> in a Inode Treeblock, 
probe it, get the value as an Lptr from map(&k)->V on the raw node type. 
*/

///in a b+tree theres 1 more key per value for inodes
///degree 2 = binary (max 2 children, 1 separator). small for easy tracing.
pub(crate) struct INode<K: Sized + Ord, I: BlockIndex, L: BlockIndex> {
    pub(crate) keys:        [K; 1],
    pub(crate) leaves:      [PtrUnion<I, L>; 2],
    pub(crate) nchildren:   u8, //occupied child slots. nkeys = nchildren-1. 0 = fresh.
    ///debug-only node height (0 = terminal/leaf, >0 = internal level). read solely by
    ///`SlotDebug::debug_render` to pick SlicePtr vs child-vaddr. never read by logic, so a
    ///stale value only affects debug output. set at construction/split sites.
    pub(crate) debug_height: u32,
}

impl<K: Ord, I: BlockIndex, L: BlockIndex> INode<K, I, L> {
    pub(crate) const DEGREE: usize = 2;

    fn nkeys(&self) -> usize { self.nchildren.saturating_sub(1) as usize }

    fn keys_slice(&self) -> &[K] { &self.keys[..self.nkeys()] }

    fn leaves_slice(&self) -> &[PtrUnion<I, L>] { &self.leaves[..self.nchildren as usize] }

    ///child vaddrs (the union's `internal` arm) for occupied child slots; `None` for the
    ///rest. reads the internal arm — valid at internal levels (see `ChildNodeIter`);
    ///terminal nodes hold `SlicePtr`s there, so callers must be height-aware.
    pub(crate) fn children_array(&self) -> [Option<I>; 2] {
        const { assert!(Self::DEGREE == 2) }
        let mut a = [None; 2];
        for i in 0..self.nchildren as usize {
            a[i] = Some(unsafe { self.leaves[i].internal });
        }
        a
    }

    ///route k to the child slot whose range contains it, return that child PtrUnion.
    ///value semantics: consumer reads .terminal.ptr for the LPtr at the bottom level,
    ///.internal for the child inode ptr. idx = match binary_search { Ok(i)=>i+1, Err(i)=>i }.
    fn map(&self, k: &K) -> Option<PtrUnion<I, L>> {
        let idx = match self.keys_slice().binary_search(k) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        (idx < self.nchildren as usize).then_some(self.leaves[idx])
    }
}

impl<K: Sized + Ord, I: BlockIndex, L: BlockIndex> SlotDebug<I> for INode<K, I, L> {
    fn debug_render(&self, tr: &Translator<I>) -> Vec<String> {
        let nc = self.nchildren as usize;
        let terminal = self.debug_height == 0;
        (0..nc)
            .map(|i| {
                if terminal {
                    //terminal: leaves hold SlicePtr<L>.
                    let sp = unsafe { self.leaves[i].terminal };
                    format!("L{}:{}", sp.ptr.as_usize(), sp.len.as_usize())
                } else {
                    //internal: leaves hold child vaddrs -> phys.
                    let cv = unsafe { self.leaves[i].internal };
                    format!("{}", tr.v2p(cv))
                }
            })
            .collect()
    }
}

///read-only height-tracking probe for the UBTree (`TreeBlock`). Holds `&'b TreeBlock`;
///`try_route` returns None at `P::MIN` (terminal — don't descend into leaves), else asks
///the node to route. `descend` decrements height. Sits above `Tree`.
pub(crate) struct InodeProbe<'b, 'a, Inner: BlockMutTrait<'a> + 'a, O: Ordering>
where Inner::T: Node<'a, Inner::P, O> {
    tree: &'b TreeBlock<'a, Inner, O>,
    pos: Inner::P,
    height: Inner::P,
}

///positioned cursor over `INode::leaves[0..nchildren]` yielding each child's
///`.internal` ptr (IPtr). Only called at internal levels (height>0).
struct ChildNodeIter<'a, I: BlockIndex, L: BlockIndex> { leaves: &'a [PtrUnion<I, L>], idx: usize }

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterBase<'a, I> for ChildNodeIter<'a, I, L> {
    fn position(&self) -> usize { self.idx }
    fn len(&self) -> usize { self.leaves.len() }
    fn cap(&self) -> usize { self.leaves.len() }
    fn prev(&mut self) { self.idx = self.idx.saturating_sub(1); }
    fn next(&mut self) { self.idx = (self.idx + 1).min(self.leaves.len()); }
    fn seek(&mut self, p: usize) { self.idx = p.min(self.leaves.len()); }
}

impl<'a, I: BlockIndex, L: BlockIndex> NodeIter<'a, I> for ChildNodeIter<'a, I, L> {
    fn current(&self) -> I { unsafe { self.leaves[self.idx].internal } }
}

///mut cursor over `INode::leaves[0..nchildren]` — `current_mut` reborrows each
///child's `.internal` (IPtr) mutably. `NodeIterMut` (not `NodeIter`): hands out
///`&mut I` per call, tied to `&mut self`, not stored.
struct ChildNodeIterMut<'a, I: BlockIndex, L: BlockIndex> { leaves: &'a mut [PtrUnion<I, L>], idx: usize }

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterBase<'a, I> for ChildNodeIterMut<'a, I, L> {
    fn position(&self) -> usize { self.idx }
    fn len(&self) -> usize { self.leaves.len() }
    fn cap(&self) -> usize { self.leaves.len() }
    fn prev(&mut self) { self.idx = self.idx.saturating_sub(1); }
    fn next(&mut self) { self.idx = (self.idx + 1).min(self.leaves.len()); }
    fn seek(&mut self, p: usize) { self.idx = p.min(self.leaves.len()); }
}

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterMut<'a, I> for ChildNodeIterMut<'a, I, L> {
    fn current_mut(&mut self) -> &mut I { unsafe { &mut self.leaves[self.idx].internal } }
}

impl<K: Ord, I: BlockIndex, L: BlockIndex> OrderedNode<I, InOrder> for INode<K, I, L> {
    ///InOrder: parent sits between child[half-1] and child[half]. New child lands in
    ///the gap between lower (parent if child_idx==half, else child[child_idx-1]) and
    ///upper (child[child_idx]); return After(lower) — find_slot searches forward.
    fn insert_position(&self, this: I, child_idx: usize) -> RelTo<I> {
        let half = Self::DEGREE / 2;
        let nc = self.nchildren as usize;
        if nc == 0 { return RelTo::After(this); }                          //fresh: anchor at parent
        if child_idx == 0 { return RelTo::Before(unsafe { self.leaves[0].internal }); }
        let lower = if child_idx == half { this } else { unsafe { self.leaves[child_idx - 1].internal } };
        RelTo::After(lower)
    }
}


impl<'a, K, I, L> Node<'a, I, InOrder> for INode<K, I, L>
where
    K: Ord + 'a,
    I: BlockIndex,
    L: BlockIndex,
{
    type K = K;
    type V = PtrUnion<I, L>;

    fn try_route<'s>(&'s self, k: &K) -> Option<usize> where 'a: 's {
        if self.nchildren == 0 { return None; }
        Some(route_idx(self, k))
    }

    fn lookup<'s>(&'s self, query: &K) -> Option<impl NodeIter<'s, I>> where 'a: 's {
        let idx = route_idx(self, query);
        (idx < self.nchildren as usize).then_some(ChildNodeIter { leaves: self.leaves_slice(), idx })
    }

    fn keys<'s>(&'s self) -> impl NodeIter<'s, &'s K> where 'a: 's {
        SliceNodeIter { slice: self.keys_slice(), idx: 0 }
    }

    fn children<'s>(&'s self) -> impl NodeIter<'s, I> where 'a: 's {
        ChildNodeIter { leaves: self.leaves_slice(), idx: 0 }
    }

    fn children_mut<'s>(&'s mut self) -> impl NodeIterMut<'s, I> where 'a: 's {
        ChildNodeIterMut { leaves: &mut self.leaves[..self.nchildren as usize], idx: 0 }
    }

    fn sibling_ptrs(&mut self) -> Option<(&mut I, &mut I)> { todo!() }
    fn parent_ptr(&mut self) -> Option<&mut I> { todo!() }
    fn self_ptr(&mut self) -> Option<&mut I> { todo!() }
    fn remove_child(&mut self, _k: &K, _child_idx: usize) { todo!() }
    fn degree() -> usize { Self::DEGREE }

    fn update_child(&mut self, child_idx: usize, new_p: I) { self.leaves[child_idx].internal = new_p; }

    fn clear_child(&mut self, _child_idx: usize) { todo!() }
}


impl<'b, 'a, Inner, O> TreePos<Inner::P> for InodeProbe<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn position(&self) -> Inner::P { self.pos }
    fn set_position(&mut self, p: Inner::P) { self.pos = p; }
}

impl<'b, 'a, Inner, O> TreeRoute<'b, 'a, TreeBlock<'a, Inner, O>> for InodeProbe<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn try_route(&self, k: &<Inner::T as Node<'a, Inner::P, O>>::K) -> Option<usize> {
        if self.height == Inner::P::MIN { return None; }
        self.block().get(self.position()).try_route(k)
    }

    fn block(&self) -> &Inner { self.tree.inner() }
}

impl<'b, 'a, Inner, O> Probe<'b, 'a, TreeBlock<'a, Inner, O>> for InodeProbe<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn new(tree: &'b TreeBlock<'a, Inner, O>) -> Self {
        Self { tree, pos: tree.root(), height: *tree.meta() }
    }

    fn descend<'s>(&'s mut self, child_idx: usize) where 'a: 's {
        self.set_position(self.child_at(child_idx));
        self.height = self.height.wrapping_sub(Inner::P::ONE);
    }
}

///owning-mut height-tracking walker for the UBTree (`TreeBlock`). Holds `&'b mut
///TreeBlock` (the whole tree — so it can update root/meta on a split), the ancestor
///stack for `TreeNav`, and a `height` counter seeded from `TreeBlock::meta`; `descend`
///pushes lineage, steps, and decrements height.
pub(crate) struct InodeWalker<'b, 'a, Inner: BlockMutTrait<'a> + 'a, O: Ordering>
where Inner::T: Node<'a, Inner::P, O> {
    tree: &'b mut TreeBlock<'a, Inner, O>,
    pos: Inner::P,
    height: Inner::P,
    stack: VecDeque<(Inner::P, usize)>,
}

impl<'b, 'a, Inner, O> TreePos<Inner::P> for InodeWalker<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn position(&self) -> Inner::P { self.pos }
    fn set_position(&mut self, p: Inner::P) { self.pos = p; }
}

impl<'b, 'a, Inner, O> TreeRoute<'b, 'a, TreeBlock<'a, Inner, O>> for InodeWalker<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn try_route(&self, k: &<Inner::T as Node<'a, Inner::P, O>>::K) -> Option<usize> {
        if self.height == Inner::P::MIN { return None; }
        self.block().get(self.position()).try_route(k)
    }

    fn block(&self) -> &Inner { self.tree.inner() }
}

impl<'b, 'a, Inner, O> TreeNav<Inner::P> for InodeWalker<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn pop(&mut self) -> Option<(Inner::P, usize)> { self.stack.pop_back() }
    fn push(&mut self, parent: Inner::P, child_idx: usize) { self.stack.push_back((parent, child_idx)); }
    fn parent(&self) -> Option<(Inner::P, usize)> { self.stack.back().copied() }
    fn ascend(&mut self) {
        if let Some((pv, _)) = self.pop() {
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
        }
    }
    ///in-order (depth-first) successor. layout: left children (child[0..half]), parent,
    ///right children (child[half..k]); the parent sits between child[half-1] and
    ///child[half]. so an internal node's successor is the leftmost of its first right
    ///child (child[half]); if it has no right children it's the last in its subtree and
    ///we go up. a terminal's successor is found by ascending to the nearest ancestor
    ///where we came up from a non-last child.
    fn next(&mut self) {
        let half = <Inner::T as Node<'a, Inner::P, O>>::degree() / 2;
        eprintln!("[next] before pos={:?} h={:?} parent={:?}", self.pos, self.height, self.parent());
        if self.height > Inner::P::MIN {
            let k = self.block().get(self.pos).children().len();
            if k > half {
                self.descend(half);
                while self.height > Inner::P::MIN { self.descend(0); }
                return;
            }
        }
        loop {
            let Some((pv, j)) = self.pop() else { return; };
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
            let k = self.block().get(pv).children().len();
            //came up from child[j] of pv. pv's in-order: child[0..half], pv, child[half..k].
            let last_left = if k > half { j == half - 1 } else { j == k - 1 };
            if last_left { return; }                  //successor is pv (the parent)
            if k > half && j == k - 1 { continue; }   //last right child -> keep ascending
            self.descend(j + 1);                     //else leftmost of the next child
            while self.height > Inner::P::MIN { self.descend(0); }
            return;
        }
    }
    ///in-order (depth-first) predecessor, mirror of next. an internal node's predecessor
    ///is the rightmost of its last left child; a terminal's is the rightmost of the
    ///previous child, or its parent if it's the first right child, or the parent's
    ///predecessor if it's the first child.
    fn prev(&mut self) {
        let half = <Inner::T as Node<'a, Inner::P, O>>::degree() / 2;
        loop {
            if self.height > Inner::P::MIN {
                let k = self.block().get(self.pos).children().len();
                if k > 0 {
                    let last_left = if k > half { half - 1 } else { k - 1 };
                    self.descend(last_left);
                    while self.height > Inner::P::MIN {
                        let k2 = self.block().get(self.pos).children().len();
                        if k2 > half { self.descend(k2 - 1); } else { break; }
                    }
                    return;
                }
            }
            let Some((pv, j)) = self.pop() else { return; };
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
            if j == half { return; }    //came from first right child -> predecessor is pv
            if j > 0 {
                self.descend(j - 1);    //rightmost of the previous child
                while self.height > Inner::P::MIN {
                    let k2 = self.block().get(self.pos).children().len();
                    if k2 > half { self.descend(k2 - 1); } else { break; }
                }
                return;
            }
            //j == 0: came from first child -> predecessor is pv's predecessor (loop).
        }
    }
    fn right(&mut self) { todo!() }
    fn left(&mut self) { todo!() }
}

impl<'b, 'a, Inner, O> Walker<'b, 'a, TreeBlock<'a, Inner, O>> for InodeWalker<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn new(tree: &'b mut TreeBlock<'a, Inner, O>) -> Self {
        let pos = tree.root();
        let height = *tree.meta();
        Self { tree, pos, height, stack: VecDeque::new() }
    }

    fn block_mut(&mut self) -> &mut Inner { self.tree.inner_mut() }

    fn root(&self) -> Inner::P { self.tree.root() }
    fn set_root(&mut self, root: Inner::P) { *self.tree.root_mut() = root; }

    fn descend<'s>(&'s mut self, child_idx: usize) where 'a: 's {
        let cur = self.position();
        let child = self.child_at(child_idx);
        self.push(cur, child_idx);
        self.set_position(child);
        self.height = self.height.wrapping_sub(Inner::P::ONE);
    }
}

// ─── UBTree: union b+tree, the inode half ─────────────────────────────────────
// arena over a UniformBlock of INodes; ptr upkeep only. values are PtrUnions the
// consumer points at external leaf storage. terminal INodes store range-bucket
// ptrs (separator keys bound ranges); internal INodes route by separator to child
// inodes. insert splits proactively top-down (full root grows height via
// `insert_root`; full child splits into a sibling via `insert_child` + manual
// parent wiring). remove does not rebalance (underflow left).

const UB_CAP: usize = 4096;
pub(crate) type UBInner<K> = UniformBlock<'static, INode<K, u32, u16>, InOrder, u32, UB_CAP>;
type UBBlock<K> = TreeBlock<'static, UBInner<K>, InOrder>;

impl<K, I, L> INode<K, I, L>
where
    K: Ord + Copy + Default,
    I: BlockIndex,
    L: BlockIndex,
{
    fn empty() -> Self {
        Self {
            keys: std::array::from_fn(|_| K::default()),
            leaves: std::array::from_fn(|_| PtrUnion { internal: I::MIN }),
            nchildren: 0,
            debug_height: 0,
        }
    }

    fn is_full(&self) -> bool { self.nchildren as usize >= Self::DEGREE }

    ///install/replace the bucket for separator `k`: if `k` is an existing separator,
    ///overwrite its bucket; else insert a new separator + bucket (shift right). caller
    ///guarantees the node is not full (proactive split). leaves[0] is the permanent
    ///underflow bucket (range below the first separator); it's never moved by inserts.
    ///nchildren = #buckets = #separators + 1, so the first insert goes 0 -> 2.
    fn insert_bucket(&mut self, k: K, v: PtrUnion<I, L>) {
        debug_assert!(!self.is_full(), "insert_bucket: node full");
        let nc = self.nchildren as usize;
        if nc == 0 {
            //leaves[0] is the underflow bucket (empty from init); leaves[1] is k's.
            self.keys[0] = k;
            self.leaves[1] = v;
            self.nchildren = 2;
            return;
        }
        let nk = nc - 1;
        match self.keys[..nk].binary_search(&k) {
            Ok(i) => self.leaves[i + 1] = v,
            Err(i) => {
                self.keys.copy_within(i..nk, i + 1);
                self.leaves.copy_within(i + 1..nc, i + 2);
                self.keys[i] = k;
                self.leaves[i + 1] = v;
                self.nchildren += 1;
            }
        }
    }

    ///remove separator `k` and its bucket (leaves[i+1]); the left bucket absorbs the
    ///range. returns the removed bucket if `k` was a separator, else None.
    fn remove_bucket(&mut self, k: &K) -> Option<PtrUnion<I, L>> {
        let nk = self.nkeys();
        let i = self.keys[..nk].binary_search(k).ok()?;
        let removed = self.leaves[i + 1];
        let nc = self.nchildren as usize;
        self.keys.copy_within(i + 1..nk, i);
        self.leaves.copy_within(i + 2..nc, i + 1);
        self.nchildren -= 1;
        Some(removed)
    }

    ///split at `mid`: self keeps leaves[0..mid] (nchildren=mid), right gets
    ///leaves[mid..n]. separator keys[mid-1] moves up to the parent (the boundary
    ///between the two halves). terminal and internal split identically here — the
    ///boundary key lives in the parent, not duplicated. returns (separator, right).
    fn split_off(&mut self, mid: usize) -> (K, INode<K, I, L>) {
        let n = self.nchildren as usize;
        let sep = self.keys[mid - 1];
        let right_n = n - mid;
        let mut right = INode::empty();
        if right_n > 1 {
            right.keys[..right_n - 1].copy_from_slice(&self.keys[mid..n - 1]);
        }
        right.leaves[..right_n].copy_from_slice(&self.leaves[mid..n]);
        right.nchildren = right_n as u8;
        self.nchildren = mid as u8;
        (sep, right)
    }
}

///split a full child of the walker's current node. ORDER MATTERS: the arena insert's
///moved-ptr fixup traverses the tree, so the tree must be intact when we insert. thus:
/// (1) copy off the right half WITHOUT mutating the child (tree intact);
/// (2) `insert_child` arena-places the new sibling in-order (fixup traverses the
///     intact tree); (3) wire the parent (shift separators/child ptrs, insert the pushed
/// separator + sibling ptr); (4) only then shrink the old child to its left half.
/// returns the pushed separator (caller picks which half to descend into). parent is
/// assumed non-full (proactive top-down split invariant).
fn split_child<K>(
    walker: &mut InodeWalker<'_, 'static, UBInner<K>, InOrder>,
    child_idx: usize,
) -> K
where
    K: Ord + Copy + Default + 'static,
{
    let child_v = walker.child_at(child_idx);
    let parent_v = walker.position();
    let mid = INode::<K, u32, u16>::DEGREE / 2;
    //(1) copy off the right half; don't mutate the child.
    let (sep, right) = {
        let child = walker.block().get(child_v);
        let n = child.nchildren as usize;
        let sep = child.keys[mid - 1];
        let right_n = n - mid;
        let mut right = INode::empty();
        if right_n > 1 { right.keys[..right_n - 1].copy_from_slice(&child.keys[mid..n - 1]); }
        right.leaves[..right_n].copy_from_slice(&child.leaves[mid..n]);
        right.nchildren = right_n as u8;
        right.debug_height = child.debug_height; //sibling sits at the child's level
        (sep, right)
    };
    //(2) arena-place the new sibling in-order (insert_child descends to child_idx+1,
    //    the anchor / moved-run start; tree intact so the fixup traverses correctly).
    let sibling_v = walker
        .insert_child(child_idx + 1, right)
        .ok()
        .expect("split_child: arena full");
    //(3) wire the parent: shift separators/child ptrs right, insert sep + sibling ptr.
    let parent = walker.block_mut().get_mut(parent_v);
    let nc = parent.nchildren as usize;
    parent.keys.copy_within(child_idx..nc - 1, child_idx + 1);
    parent.leaves.copy_within(child_idx + 1..nc, child_idx + 2);
    parent.keys[child_idx] = sep;
    parent.leaves[child_idx + 1].internal = sibling_v;
    parent.nchildren += 1;
    //(4) shrink the old child to its left half (boundary key already moved to parent).
    walker.block_mut().get_mut(child_v).nchildren = mid as u8;
    sep
}

pub struct UBTree<K: Ord + Copy + Default + 'static> {
    pub(crate) tree: UBBlock<K>,
}

impl<K: Ord + Copy + Default + 'static> UBTree<K> {
    pub fn new() -> Self {
        Self { tree: TreeBlock::new(INode::empty(), 0u32) }
    }

    pub fn get(&self, k: &K) -> Option<PtrUnion<u32, u16>> {
        let mut p = InodeProbe::new(&self.tree);
        while let Some(ci) = p.try_route(k) {
            p.descend(ci);
        }
        p.current().map(k)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut PtrUnion<u32, u16>> {
        let terminal_v = {
            let mut w = InodeWalker::new(&mut self.tree);
            while let Some(ci) = w.try_route(k) {
                w.descend(ci);
            }
            w.position()
        };
        let node = self.tree.inner_mut().get_mut(terminal_v);
        let nc = node.nchildren as usize;
        let nk = node.nkeys();
        let idx = match node.keys[..nk].binary_search(k) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        (idx < nc).then(|| &mut node.leaves[idx])
    }

    pub fn insert(&mut self, k: K, v: PtrUnion<u32, u16>) {
        //split on overflow: the root only gains a child when (a) it's a terminal root and a
        //new separator is inserted (full root 0..=DEGREE-1 seps → +1 overflows), or (b) it's an
        //internal root and an immediate child split pushes a separator up. proactive splitting
        //keeps every non-root node non-full when visited, so overflow never propagates past one
        //level — the root only grows from its immediate child splitting. a full root that
        //gains no child this insert (overwrite, or descent into a non-full child) is left alone.
        //the outer loop only re-enters after a split_root (which rewrites the root); the inner
        //loop is the persistent-walker descent.
        'split: loop {
            let mut walker = InodeWalker::new(&mut self.tree);
            loop {
                if walker.height == u32::MIN {
                    let new_sep = walker.current().keys_slice().binary_search(&k).is_err();
                    if walker.current().is_full() && new_sep {
                        drop(walker);
                        self.split_root();
                        continue 'split;
                    }
                    walker.current_mut().insert_bucket(k, v);
                    return;
                }
                let mut child_idx = route_idx(walker.current(), &k);
                let child_v = walker.child_at(child_idx);
                if walker.block().get(child_v).is_full() {
                    if walker.current().is_full() {
                        drop(walker);
                        self.split_root();
                        continue 'split;
                    }
                    let sep = split_child(&mut walker, child_idx);
                    if k >= sep {
                        child_idx += 1;
                    }
                }
                walker.descend(child_idx);
            }
        }
    }

    ///remove separator `k` and its bucket. no rebalancing — underfull nodes are left.
    pub fn remove(&mut self, k: &K) -> Option<PtrUnion<u32, u16>> {
        let terminal_v = {
            let mut w = InodeWalker::new(&mut self.tree);
            while let Some(ci) = w.try_route(k) {
                w.descend(ci);
            }
            w.position()
        };
        self.tree.inner_mut().get_mut(terminal_v).remove_bucket(k)
    }

    ///split a full root: split it in place, place the right half as a sibling after the
    ///old root (pinned so it stays put), then place a new root (pre-wired to [old root,
    ///sibling]) after the old root as well and reassign tree.root + bump height. no-slide
    ///placement (spread-on-slide) keeps every vaddr stable across the spreads, so the
    ///new root's child ptrs stay valid. both placements anchor at the old root and go
    ///after it — spread opens adjacent Nones between/after, so neither slides the other.
    ///split a full root: copy off the right half (tree intact), promote a new root above
    ///the old one via `insert_root` (swaps the new root into the root vaddr, moves the old
    ///root aside, returns its new vaddr), place the right half as the new root's child 1
    ///via `insert_child`, wire the new root to [old root, sibling] + the pushed separator,
    ///bump height, then shrink the old root to its left half. tree.root() (unchanged
    ///vaddr) ends up holding the new root.
    fn split_root(&mut self) {
        let old_root_v = self.tree.root();
        let old_h = *self.tree.meta();
        let mid = INode::<K, u32, u16>::DEGREE / 2;
        //(1) copy off the right half (don't mutate the old root).
        let (sep, right) = {
            let r = self.tree.inner().get(old_root_v);
            let n = r.nchildren as usize;
            let sep = r.keys[mid - 1];
            let right_n = n - mid;
            let mut right = INode::empty();
            if right_n > 1 { right.keys[..right_n - 1].copy_from_slice(&r.keys[mid..n - 1]); }
            right.leaves[..right_n].copy_from_slice(&r.leaves[mid..n]);
            right.nchildren = right_n as u8;
            right.debug_height = old_h; //sibling sits at the old root's (now child) level
            (sep, right)
        };
        let mut walker = InodeWalker::new(&mut self.tree);
        //(2) promote the new root (unwired) above the old root.
        let old_root_new_v = walker
            .insert_root(old_root_v, INode::empty())
            .ok()
            .expect("split_root: insert_root");
        //(3) place the right half as the new root's child 1.
        let sibling_v = walker
            .insert_child(1, right)
            .ok()
            .expect("split_root: sibling");
        //(4) wire the new root (at tree.root() == old_root_v, now holding the new root).
        let root_v = self.tree.root();
        let new_root = self.tree.inner_mut().get_mut(root_v);
        new_root.keys[0] = sep;
        new_root.leaves[0] = PtrUnion { internal: old_root_new_v };
        new_root.leaves[1] = PtrUnion { internal: sibling_v };
        new_root.nchildren = 2;
        new_root.debug_height = old_h + 1; //new root sits one level above its children
        *self.tree.meta_mut() = self.tree.meta().wrapping_add(u32::ONE);
        //(5) shrink the old root (now at old_root_new_v) to its left half.
        self.tree.inner_mut().get_mut(old_root_new_v).nchildren = mid as u8;
    }
}
