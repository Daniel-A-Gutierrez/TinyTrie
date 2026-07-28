use crate::RelTo;
use crate::block::{AllocStrat, BlockBase, RawBlock};
use crate::{index::BlockIndex, store::Store, translator::Translator};
use std::marker::PhantomData;
///tree layout + root placement. RawBlock carries no ordering; this is a tree-tier
///concern. root_position picks the root vaddr given the block's first/last occupied
///vaddrs (supplied by BlockBase); the block owns them.
pub trait TreeOrdering: 'static {
    fn root_position<P: BlockIndex>(first: P, last: P) -> P;
}
pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
///fast for iterating over leaves, splitting is difficult.
///ordering requires node receiving child to already have at least 1 child, except for the root.
//impl TreeOrdering for BFO {}
///like preorder but reversed
//impl TreeOrdering for PostOrder {}
///easiest to split, iteration OK
impl TreeOrdering for InOrder {
    //in-order: root sits at the address anchor.
    fn root_position<P: BlockIndex>(_first: P, _last: P) -> P {
        P::MIDPOINT
    }
}
///lookup only goes forward, next element is child or sibling, can be fast for chains.
//impl TreeOrdering for PreOrder {}
/// blocks that store a type that impls Node support automatic internal navigation by default.
///
/// for a b+tree, only the cursor knows which discriminant V is so we have to be 'dumb'.
/// D = DEGREE , the maximum number of children of a node.
trait Node<'a, O: TreeOrdering>: Sized + 'a + OrderedNode<Self::P, O> {
    type K: Sized + 'a;
    type V: Sized + 'a;
    type P: BlockIndex;
    fn lookup(&self, query: &Self::K) -> impl NodeIter<'a, Self::P>;
    fn try_lookup(&self, query: &Self::K) -> impl NodeIter<'a, Option<Self::P>>;
    fn keys(&self) -> impl NodeIter<'a, &'a Self::K>;
    fn values(&self) -> impl NodeIter<'a, &'a Self::V>;
    fn pairs(&self) -> impl NodeIter<'a, (&'a Self::K, &'a Self::V)>;
    fn children(&self) -> impl NodeIter<'a, Self::P>;
    fn children_mut(&mut self) -> impl NodeIterMut<'a, &mut Self::P>;
    fn sibling_ptrs(&mut self) -> Option<(&mut Self::P, &mut Self::P)>;
    fn parent_ptr(&mut self) -> Option<&mut Self::P>;
    fn self_ptr(&mut self) -> Option<&mut Self::P>;
    fn remove(&mut self, k: &Self::K, child_idx: usize); //if node has no keys afterward it should be removed.
    fn degree() -> usize; //the max degree of the node type, how many children it can possibly have.
}
///node may store its elements sparse, next/prev isnt necessarily position +- 1;
trait NodeIterBase<'a, T> {
    fn position(&self) -> usize;
    fn len(&self) -> usize; //number of elements, not necessarily max position.
    fn cap(&self) -> usize; //max in bounds position + 1.
    fn prev(&mut self);
    fn next(&mut self);
    fn seek(&mut self, p: usize);
}
trait NodeIter<'a, T>: NodeIterBase<'a, T> {
    fn current(&self) -> T;
}
trait NodeIterMut<'a, T>: NodeIter<'a, T> {
    fn current_mut(&mut self) -> T;
}
///tree walker owns a &mut Block<T : Node, O : Ordering .etc>
trait TreeWalker<'a, B: Tree<'a>>: TreeProbe<'a, B> {
    fn pop(&mut self) -> Option<(usize, usize)>; //take the parent position off the ancestor stack.
    fn push(&mut self, position: usize); //put a new physical position in the ancestor stack.
    fn parent(&self) -> Option<(usize, usize)>; //view the top of the ancestor stack (parent, child_idx).
    fn ascend(&mut self); //goto parent
    fn next(&mut self); //go to next node in the defined ordering
    fn prev(&mut self); //go to prev node in the defined ordering
    fn right(&mut self); //go to prev sibling/cousin, skipping parent. 
    fn left(&mut self); //go to next sibling/cousin, skipping parent
}
trait TreeWalkerMut<'a, B: Tree<'a>>: TreeProbeMut<'a, B> + TreeWalker<'a, B> {
    ///insert a new node in the arena as a child of current.
    ///does not modify current, aside from pointer maintenance.
    fn insert_child(&mut self, child_idx: usize, node: B::T);
    ///remove current from the arena. It must not have children.
    fn remove(&mut self) -> B::T;
}
trait TreeProbe<'a, B: Tree<'a>> {
    fn position(&self) -> usize; // physical position of current node
    fn current(&self) -> Option<&B::V>; //current node
    fn descend(&mut self, child_idx: usize); //goto current.vals[child_idx];
}
trait TreeProbeMut<'a, B: Tree<'a>>: TreeProbe<'a, B> {
    fn current_mut(&mut self) -> &mut B::V;
}
/// type that the block stores, key type, value type, ptrs type, address translator, ordering, store
trait Tree<'a>: Sized + OrderedBlock<'a, Self::T, Self::P, Self::O, Self::S> {
    type T: Node<'a, Self::O>;
    type K: Sized;
    type V: Sized;
    type P: BlockIndex;
    type A: AllocStrat;
    type O: TreeOrdering;
    type S: Store<'a, Self::T> + 'a;
    fn root(&self) -> impl TreeWalker<'a, Self>;
    fn walk_to(&self, k: Self::K) -> impl TreeWalker<'a, Self>;
    fn probe(&self, k: Self::K) -> impl TreeProbe<'a, Self>;
    fn insert<W>(
        &mut self,
        walker: W,
        child_idx: usize,
        node: Self::T,
    ) -> Result<Self::P, Self::T>
    where
        W: TreeWalker<'a, Self>;
    fn remove<W>(&mut self, walker: W) -> Option<Self::T>
    where W: TreeWalker<'a, Self>;
}
trait OrderedBlock<'a, T: Sized + 'a, P: BlockIndex, O: TreeOrdering, S: Store<'a, T>>:
    BlockBase<'a, T, P, S>
{
    //block supplies first/last (its occupied vaddrs); O picks among them / midpoint.
    //empty block → midpoint (the anchor, where the root will live).
    fn root_vaddr(&self) -> P {
        O::root_position(
            self.first_vaddr().unwrap_or(P::MIDPOINT),
            self.last_vaddr().unwrap_or(P::MIDPOINT),
        )
    }
}
trait OrderedNode<P: BlockIndex, O: TreeOrdering> {
    fn insert_position(&self, child_idx: usize) -> RelTo<usize>;
}
struct TreeBlock<'a, T, K, V, P, A, O, S>
where
    T: Node<'a, O>,
    K: Sized,
    V: Sized,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{
    ///private: callers must go through TreeBlock so rewiring can't be bypassed.
    raw: RawBlock<'a, T, P, A, S>,
    _p:  PhantomData<(K, V, O)>,
}
//BlockBase via the two accessors — the read surface forwards through raw's impl.
impl<'a, T, K, V, P, A, O, S> BlockBase<'a, T, P, S> for TreeBlock<'a, T, K, V, P, A, O, S>
where
    T: Node<'a, O>,
    K: Sized + 'a,
    V: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{
    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        self.raw.store()
    }
    fn translator<'b>(&'b self) -> &'b Translator<P> {
        self.raw.translator()
    }
}
impl<'a, T, K, V, P, A, O, S> OrderedBlock<'a, T, P, O, S>
    for TreeBlock<'a, T, K, V, P, A, O, S>
where
    T: Node<'a, O>,
    K: Sized + 'a,
    V: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{
}
impl<'a, T, K, V, P, A, O, S> Tree<'a> for TreeBlock<'a, T, K, V, P, A, O, S>
where
    T: Node<'a, O>,
    K: Sized + 'a,
    V: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{
    type A = A;
    type T = T;
    type K = K;
    type V = V;
    type P = P;
    type O = O;
    type S = S;
    fn insert<W>(
        &mut self,
        walker: W,
        child_idx: usize,
        node: Self::T,
    ) -> Result<Self::P, Self::T>
    where
        W: TreeWalker<'a, Self>,
    {
        todo!()
    }
    fn probe(&self, k: Self::K) -> impl TreeProbe<'a, Self> {
        todo!()
    }
    fn remove<W>(&mut self, walker: W) -> Option<Self::T>
    where W: TreeWalker<'a, Self> {
        todo!()
    }
    fn root(&self) -> impl TreeWalker<'a, Self> {
        todo!()
    }
    fn walk_to(&self, k: Self::K) -> impl TreeWalker<'a, Self> {
        todo!()
    }
}
