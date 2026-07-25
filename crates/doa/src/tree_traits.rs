use crate::{block::Ordering, index::BlockIndex, store::Store, translator::AddressTranslator};
pub trait TreeOrdering: Ordering {}
pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
///fast for iterating over leaves, splitting is difficult.
impl Ordering for BFO {}
//impl TreeOrdering for BFO {}
///like preorder but reversed
impl Ordering for PostOrder {}
//impl TreeOrdering for PostOrder{}
///easiest to split, iteration OK
impl Ordering for InOrder {}
impl TreeOrdering for InOrder {}
///lookup only goes forward, next element is child or sibling, can be fast for chains.
impl Ordering for PreOrder {}
//impl TreeOrdering for PreOrder {}
/// blocks that store a type that impls Node support automatic internal navigation by default.
///
/// for a b+tree, only the cursor knows which discriminant V is so we have to be 'dumb'.
/// D = DEGREE , the maximum number of children of a node.
trait Node<'a>: Sized + 'a {
    type K: Sized + 'a;
    type V: Sized + 'a;
    type P: BlockIndex + 'a;
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
    fn insert(&mut self, k: Self::K, p: Self::P, child_idx: usize);
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
trait TreeWalker<'a, B: TreeBlock<'a>>: TreeProbe<'a, B> {
    fn pop(&mut self) -> Option<(usize, usize)>; //take the parent position off the ancestor stack.
    fn push(&mut self, position: usize); //put a new physical position in the ancestor stack.
    fn parent(&self) -> Option<(usize, usize)>; //view the top of the ancestor stack (parent, child_idx).
    fn ascend(&mut self); //goto parent
    fn next(&mut self); //go to next node in the defined ordering
    fn prev(&mut self); //go to prev node in the defined ordering
    fn right(&mut self); //go to prev sibling/cousin, skipping parent. 
    fn left(&mut self); //go to next sibling/cousin, skipping parent
}
trait TreeWalkerMut<'a, B: TreeBlock<'a>>: TreeProbeMut<'a, B> + TreeWalker<'a, B> {
    ///insert a new node in the arena as a child of current.
    ///does not modify current, aside from pointer maintenance.
    fn insert_child(&mut self, child_idx: usize, node: B::T);
    ///remove current from the arena. It must not have children.
    fn remove(&mut self) -> B::T;
}
trait TreeProbe<'a, B: TreeBlock<'a>> {
    fn position(&self) -> usize; // physical position of current node
    fn current(&self) -> Option<&B::V>; //current node
    fn descend(&mut self, child_idx: usize); //goto current.vals[child_idx];
}
trait TreeProbeMut<'a, B: TreeBlock<'a>>: TreeProbe<'a, B> {
    fn current_mut(&mut self) -> &mut B::V;
}
/// type that the block stores, key type, value type, ptrs type, address translator, ordering, store
trait TreeBlock<'a>: Sized {
    type T: Node<'a>;
    type K: Sized;
    type V: Sized;
    type P: BlockIndex;
    type A: AddressTranslator<Self::P>;
    type O: Ordering;
    type S: Store<'a, Self::T> + 'a;
    fn root(&self) -> impl TreeWalker<'a, Self>;
    fn walk_to(&self, k: Self::K) -> impl TreeWalker<'a, Self>;
    fn probe(&self, k: Self::K) -> impl TreeProbe<'a, Self>;
}
