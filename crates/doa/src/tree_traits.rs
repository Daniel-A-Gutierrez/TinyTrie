use std::marker::PhantomData;
use crate::RelTo;
use crate::{block::Ordering, index::BlockIndex, store::Store, translator::AddressTranslator};
use crate::block::{AllocStrat, Block, BlockBase};
///ordering-dependent tree behavior. root_position picks the root vaddr given
///the block's first/last occupied vaddrs (supplied by BlockBase); the block owns them.
pub trait TreeOrdering : Ordering {
    fn root_position<P: BlockIndex>(first: P, last: P) -> P;
}
pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
///fast for iterating over leaves, splitting is difficult.
///ordering requires node receiving child to already have at least 1 child, except for the root. 
impl Ordering for BFO {}
//impl TreeOrdering for BFO {}
///like preorder but reversed
impl Ordering for PostOrder {}
//impl TreeOrdering for PostOrder{}
///easiest to split, iteration OK
impl Ordering for InOrder {}
impl TreeOrdering for InOrder {
    //in-order: root sits at the address anchor.
    fn root_position<P: BlockIndex>(_first: P, _last: P) -> P { P::MIDPOINT }
}
///lookup only goes forward, next element is child or sibling, can be fast for chains.
impl Ordering for PreOrder {}
//impl TreeOrdering for PreOrder {}
/// blocks that store a type that impls Node support automatic internal navigation by default.
///
/// for a b+tree, only the cursor knows which discriminant V is so we have to be 'dumb'.
/// D = DEGREE , the maximum number of children of a node.
trait Node<'a, O: TreeOrdering>: Sized + 'a + OrderedNode<Self::P, O> {
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
trait Tree<'a>: Sized + OrderedBlock<'a, Self::T, Self::P, Self::O> {
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
    fn insert<W>(&mut self, walker : W , child_idx : usize, node : Self::T)->Result<Self::P, Self::T>
        where W : TreeWalker<'a, Self>;
    fn remove<W>(&mut self, walker : W)->Option<Self::T>
        where W : TreeWalker<'a, Self>;
    
}

trait OrderedBlock<'a, T: Sized + 'a, P: BlockIndex, O : TreeOrdering>: BlockBase<'a, T, P> {
    //block supplies first/last (its occupied vaddrs); O picks among them / midpoint.
    //empty block → midpoint (the anchor, where the root will live).
    fn root_vaddr(&self) -> P {
        O::root_position(
            self.first_vaddr().unwrap_or(P::MIDPOINT),
            self.last_vaddr().unwrap_or(P::MIDPOINT),
        )
    }
}

trait OrderedNode<P: BlockIndex, O : TreeOrdering> {
    fn insert_position(&self, child_idx : usize) -> RelTo<usize>;
}

struct TreeBlock<'a,T,K,V,P,A,O,S> where
    T: Node<'a, O>,
    K : Sized,
    V : Sized,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a {
    raw : Block<'a,T,P,O,A,S>,
    _p : PhantomData<(K,V)>
}

impl<'a,T,K,V,P,A,O,S> BlockBase<'a, T, P> for TreeBlock<'a,T,K,V,P,A,O,S>
where
    T: Node<'a, O>,
    K: Sized,
    V: Sized,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{
    fn get(&self, ptr: P) -> &T { BlockBase::get(&self.raw, ptr) }
    fn get_mut(&mut self, ptr: P) -> &mut T { BlockBase::get_mut(&mut self.raw, ptr) }
    fn first_vaddr(&self) -> Option<P> { BlockBase::first_vaddr(&self.raw) }
    fn last_vaddr(&self) -> Option<P> { BlockBase::last_vaddr(&self.raw) }
    fn v2p(&self, virt: P) -> usize { BlockBase::v2p(&self.raw, virt) }
    fn p2v(&self, phys: usize) -> P { BlockBase::p2v(&self.raw, phys) }
    fn vdist(&self, v1: P, v2: P) -> usize { BlockBase::vdist(&self.raw, v1, v2) }
    fn occupied(&self) -> usize { BlockBase::occupied(&self.raw) }
    fn len(&self) -> usize { BlockBase::len(&self.raw) }
    fn cap(&self) -> usize { BlockBase::cap(&self.raw) }
    fn max_capacity(&self) -> usize { BlockBase::max_capacity(&self.raw) }
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b T> + 'b
    where
        T: 'b,
        'a: 'b,
    {
        BlockBase::iter(&self.raw)
    }
    fn cursor<'b>(&'b self) -> impl crate::store::Cursor<'b, T> + 'b
    where
        T: 'b,
        'a: 'b,
    {
        BlockBase::cursor(&self.raw)
    }
}

impl<'a,T,K,V,P,A,O,S> OrderedBlock<'a, T, P, O> for TreeBlock<'a,T,K,V,P,A,O,S>
where
    T: Node<'a, O>,
    K: Sized,
    V: Sized,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
{}

impl<'a,T,K,V,P,A,O,S> Tree<'a> for TreeBlock<'a,T,K,V,P,A,O,S> where
    T: Node<'a, O>,
    K: Sized,
    V: Sized,
    P: BlockIndex,
    A: AllocStrat,
    O: TreeOrdering,
    S: Store<'a, T> + 'a {
    type A=A; type T=T; type K=K; type V=V; type P=P; type O=O; type S=S;

    fn insert<W>(&mut self, walker : W , child_idx : usize, node : Self::T)->Result<Self::P, Self::T>
        where W : TreeWalker<'a, Self> {
        
    }
    fn probe(&self, k: Self::K) -> impl TreeProbe<'a, Self> {
        
    }
    fn remove<W>(&mut self, walker : W)->Option<Self::T>
        where W : TreeWalker<'a, Self>
    {
        
    }
    fn root(&self) -> impl TreeWalker<'a, Self> {
        
    }
    fn walk_to(&self, k: Self::K) -> impl TreeWalker<'a, Self> {
        
    }
}