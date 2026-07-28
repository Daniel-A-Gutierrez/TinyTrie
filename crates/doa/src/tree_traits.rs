use crate::RelTo;
use crate::block::{AllocStrat, BlockMutTrait, BlockTrait, RawBlock};
use crate::{index::BlockIndex, store::Store, translator::Translator};
use std::marker::PhantomData;

///tree layout. RawBlock carries no ordering; this is a tree-tier concern. the root
///vaddr is stored on the block (set at construction / split), not derived — bit
///rotation in the translator makes first/last insufficient to recover it.
pub trait TreeOrdering: 'static {}

pub struct BFO;

pub struct InOrder;

pub struct PreOrder;

pub struct PostOrder;

///easiest to split, iteration OK
impl TreeOrdering for InOrder {}

trait OrderedBlock<'a, T: Sized + 'a, P: BlockIndex, O: TreeOrdering, S: Store<'a, T> + 'a>:
    BlockTrait<'a, T, P, S>
{
    fn root_vaddr(&self) -> P;
}

trait OrderedNode<P: BlockIndex, O: TreeOrdering> {
    ///vaddr the new child at child_idx should be placed before/after.
    fn insert_position(&self, this: P, child_idx: usize) -> RelTo<P>;
}

/// blocks that store a type that impls Node support automatic internal navigation by default.
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

    ///usize->P slot write: set the child ptr at child_idx. the only setter the
    ///block-tier needs; parent/sibling rewiring uses the NodeIter<&mut P> accessors.
    fn update_child(&mut self, child_idx: usize, new_p: Self::P);

    ///drop the child ptr at child_idx (for remove).
    fn clear_child(&mut self, child_idx: usize);
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

    ///take the ancestor stack: (parent vaddr, child_idx taken).
    fn pop(&mut self) -> Option<(B::P, usize)>;

    ///push (parent vaddr, child_idx) onto the ancestor stack.
    fn push(&mut self, parent: B::P, child_idx: usize);

    ///view the top of the ancestor stack (parent vaddr, child_idx).
    fn parent(&self) -> Option<(B::P, usize)>;

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

    fn position(&self) -> B::P; //current node vaddr

    fn current(&self) -> Option<&B::V>; //current node

    fn descend(&mut self, child_idx: usize); //goto current.children[child_idx];
}

trait TreeProbeMut<'a, B: Tree<'a>>: TreeProbe<'a, B> {

    fn current_mut(&mut self) -> &mut B::V;
}

/// type that the block stores, ptrs type, address translator, ordering, store.
/// K/V come from T (Node); no separate K/V params.
trait Tree<'a>: Sized + OrderedBlock<'a, Self::T, Self::P, Self::O, Self::S> {
    type T: Node<'a, Self::O>;
    type K: Sized;
    type V: Sized;
    type P: BlockIndex;
    type A: AllocStrat<Self::P>;
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

struct TreeBlock<'a, T, A, O, S, B>
where
    T: Node<'a, O>,
    A: AllocStrat<T::P>,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
    B : BlockMutTrait<'a,T,T::P,A,S> +'a,
{

    ///private: callers go through TreeBlock.
    inner:  B,
    ///root vaddr. stored; rotation makes it underivable from first/last.
    root: T::P,
    _p:   PhantomData<(O,A,S)>,
}

//BlockTrait via the two accessors — the read surface forwards through raw's impl.
impl<'a, T, A, O, S, B> BlockTrait<'a, T, T::P, S> for TreeBlock<'a, T, A, O, S, B>
where
    T: Node<'a, O>,
    A: AllocStrat<T::P>,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
    B : BlockMutTrait<'a,T,T::P,A,S> +'a,

{

    fn store<'b>(&'b self) -> &'b S
    where 'a: 'b {
        self.inner.store()
    }

    fn translator<'b>(&'b self) -> &'b Translator<T::P> {
        self.inner.translator()
    }
}

impl<'a, T, A, O, S, B> OrderedBlock<'a, T, T::P, O, S>
    for TreeBlock<'a, T, A, O, S, B>
where
    T: Node<'a, O>,
    A: AllocStrat<T::P>,
    O: TreeOrdering,
    S: Store<'a, T> + 'a,
    B : BlockMutTrait<'a,T,T::P,A,S> +'a,
{
    fn root_vaddr(&self) -> T::P {
        self.root
    }
}

// impl<'a, T, A, O, S, B> Tree<'a> for TreeBlock<'a, T, A, O, S, B>
// where
//     T: Node<'a, O>,
//     A: AllocStrat<T::P>,
//     O: TreeOrdering,
//     S: Store<'a, T> + 'a,
//     B : BlockMutTrait<'a,T,T::P,A,S> +'a,
// {
//     type A = A;
//     type T = T;
//     type K = T::K;
//     type V = T::V;
//     type P = T::P;
//     type O = O;
//     type S = S;

//     fn insert<W>(
//         &mut self,
//         walker: W,
//         child_idx: usize,
//         node: Self::T,
//     ) -> Result<Self::P, Self::T>
//     where
//         W: TreeWalker<'a, Self>,
//     {
//         let parent_v = walker.position();
//         let rel = self.inner.get(parent_v).insert_position(parent_v, child_idx);
//         let (anchor, dir) = match rel {
//             RelTo::Before(p) => (p, false),
//             RelTo::After(p) => (p, true),
//         };
//         //only the root is pinned; the parent may displace.
//         let pin = Some(self.root);
//         let ms = match self.inner.find_insert_slot(anchor, dir, pin) {
//             Some(ms) => ms,
//             None => return Err(node),
//         };
//         let new_p = self.p2v(ms.to);
//         //fixup(parent_v, child_idx, new_p): rewire inbound ptrs BEFORE slide_none.
//         //  parent.update_child(child_idx, new_p); new child's parent/sibling via
//         //  the NodeIter<&mut P> accessors. TODO: remap children displaced by the
//         //  slide (their vaddrs change) — needs the slide range; deferred.
//         let slot = self.inner.slide_none(ms, pin);
//         self.inner.insert(node, slot);
//         Ok(new_p)
//     }

//     fn remove<W>(&mut self, walker: W) -> Option<Self::T>
//     where
//         W: TreeWalker<'a, Self>,
//     {
//         let cur_v = walker.position();
//         //None => removing the root; caller handles (no parent to clear).
//         let (parent_v, child_idx) = walker.parent()?;
//         let v = self.inner.remove(cur_v);
//         self.inner.get_mut(parent_v).clear_child(child_idx);
//         Some(v)
//     }

//     // fn probe(&self, k: Self::K) -> impl TreeProbe<'a, Self> {
//     //     todo!()
//     // }

//     // fn root(&self) -> impl TreeWalker<'a, Self> {
//     //     todo!()
//     // }

//     // fn walk_to(&self, k: Self::K) -> impl TreeWalker<'a, Self> {
//     //     todo!()
//     // }
// }