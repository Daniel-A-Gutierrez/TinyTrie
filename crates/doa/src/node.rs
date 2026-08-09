//static is temporary, need to use lifetimes if i want interned keys/values in a side buf.

use crate::index::BlockIndex;

//default shorthand for stored types
pub trait D: 'static + Sized {}
impl<T> D for T where T: 'static + Sized {}

//default shorthand for iterator types
pub trait DoubleExact: DoubleEndedIterator + ExactSizeIterator {}
impl<I: DoubleEndedIterator + ExactSizeIterator> DoubleExact for I {}

pub trait Node {
    type K: D;
    type V: D;
    type P: BlockIndex;
    //maximum number of children per node (relevant for in-order ordering)
    const DEGREE: usize;
}

///kind-free parent pointer. the `UnionNode` wrapper carries it as a direct field;
///the bare union and the variants do not. a node that is a parent is always an
///inode, so its children are accessed with the variant already known from depth —
/// which is what makes a stackless run-parent-fixup possible.
pub trait HasParent<P: BlockIndex> {
    fn parent(&self) -> P;
    fn set_parent(&mut self, p: P);
}

// pub trait NodeUnion {
//     type K : D;
//     type V : D;
//     type P : BlockIndex;
//     type INodeT : Node;
//     type LNodeT : Node;
//     //maximum number of children per node (relevant for in-order ordering)
//     unsafe fn as_inode(&self)->&Self::INodeT  {todo!()}
//     unsafe fn as_lnode(&self)->&Self::LNodeT {todo!()}
// }

// pub trait NodeEnum {
//     type K : D;
//     type V : D;
//     type P : BlockIndex;
//     type INodeT : Node;
//     type LNodeT : Node;
//     fn inner(&self)->&EnumNode<Self::INodeT,Self::LNodeT> {todo!()}
//     fn inner_mut(&mut self)->&mut EnumNode<Self::INodeT,Self::LNodeT> {todo!()}
// }

///nodes which are an enum over an Inode and a LeafNode
pub enum EnumNode<I: Node, L: Node> {
    INode(I),
    LNode(L),
}
pub enum EnumRef<P> {
    INodePtr(P),
    LNodePtr(P),
}
///Nodes which are unions but somehow store the variant type of their children
///functionally not quite complete, add to as necessary.
pub trait TaggedChildNode: Node {
    // Extends INode logically, cant due to specialization
    type INode: Node;
    type LNode: Node;
    fn child(&self, idx: usize) -> EnumRef<Self::P>;
    fn children(&self) -> impl DoubleExact<Item = EnumRef<Self::P>>;
}

///bare untagged union of inode/lnode; variant is external (height-discriminated).
///never `HasParent` — parent lives on the `UnionNode` wrapper.
pub union OrphanUnionNode<I, L>
where
    I: Node + Copy,
    L: Node + Copy,
{
    pub inode: I,
    pub lnode: L,
}

impl<K, V, P, I, L> Node for OrphanUnionNode<I, L>
where
    K: D,
    V: D,
    P: BlockIndex,
    I: Node<K = K, V = V, P = P> + Copy,
    L: Node<K = K, V = V, P = P> + Copy,
{
    type K = K;
    type V = V;
    type P = P;
    const DEGREE: usize = I::DEGREE;
}

///`OrphanUnionNode` + a hoisted `parent` field. parent is kind-free (a direct
///field, no union variant needed) so a stackless walker can read a moved node's
///parent to fix stale child pointers without an ancestor stack.
pub struct UnionNode<I, L>
where
    I: Node + Copy,
    L: Node<P = I::P> + Copy,
{
    pub orphan: OrphanUnionNode<I, L>,
    pub parent: I::P,
}

impl<I, L> HasParent<I::P> for UnionNode<I, L>
where
    I: Node + Copy,
    L: Node<P = I::P> + Copy,
{
    fn parent(&self) -> I::P {
        self.parent
    }
    fn set_parent(&mut self, p: I::P) {
        self.parent = p;
    }
}

impl<K, V, P, I, L> Node for UnionNode<I, L>
where
    K: D,
    V: D,
    P: BlockIndex,
    I: Node<K = K, V = V, P = P> + Copy,
    L: Node<K = K, V = V, P = P> + Copy,
{
    type K = K;
    type V = V;
    type P = P;
    const DEGREE: usize = I::DEGREE;
}

impl<K, V, P, I, L> Node for EnumNode<I, L>
where
    K: D,
    V: D,
    P: BlockIndex,
    I: Node<K = K, V = V, P = P> + Copy,
    L: Node<K = K, V = V, P = P> + Copy,
{
    type K = K;
    type V = V;
    type P = P;
    const DEGREE: usize = I::DEGREE;
}

// ///nodes which are an untagged union over an Inode and a LeafNode, discriminated by height or reftag.
// impl<I,L,K,V,P> NodeUnion for UnionNode<I,L>
// where
//     I : Node<K=K,V=V,P=P> + Copy,
//     L : Node<K=K,V=V,P=P> + Copy,
//     K : D, V : D, P : BlockIndex
//     {
//     type INodeT = I;
//     type LNodeT = L;
//     type K = K; type V = V; type P = P;
//     unsafe fn as_inode(&self)->&I  {unsafe{&self.inode}}
//     unsafe fn as_lnode(&self)->&L  {unsafe{&self.lnode}}
// }

///nodes which store values associated with keys, and no internal pointers within the block.
pub trait LNode<K, V>: Node
where
    K: Sized + 'static,
    V: Sized + 'static,
{
    fn values(&self) -> impl DoubleExact<Item = &V>;
    fn pairs(&self) -> impl DoubleExact<Item = (&K, &V)>;
    fn keys(&self) -> impl DoubleExact<Item = &K>;
    fn insert(&mut self, k: K, v: V) -> usize;
    fn remove(&mut self, pos: usize) -> (K, V);
}

///nodes which store a single value, which may be the key , (K,V), or a distinct thing.
pub trait ValueNode<V>: Node {
    fn value(&self) -> V;
}

///node that can split. right half goes into blank
pub trait SplittableNode<K>: Default {
    fn split_into(&mut self, blank: &mut Self) -> K;
}

pub trait INode: Node {
    fn keys(&self) -> impl DoubleExact<Item = &Self::K>;
    fn try_route(&self, k: &Self::K) -> Option<usize>;
    fn child(&self, child_idx: usize) -> &Self::P;
    fn children(&self) -> impl DoubleExact<Item = &Self::P>;
    //returns child_idx of new child
    fn insert_child(&mut self, child_addr: Self::P, child_key: Self::K) -> usize;
    fn remove_child(&mut self, child_key: &Self::K) -> Option<(Self::K, Self::P)>;
}

pub trait IVNode: Node {
    fn keys(&self) -> impl DoubleExact<Item = &Self::K>;
    fn try_route(&self, k: &Self::K) -> Option<usize>;
    fn child(&self, child_idx: usize) -> &Self::P;
    fn children(&self) -> impl DoubleExact<Item = &Self::P>;
    fn pairs(&self) -> impl DoubleExact<Item = (&Self::K, &Self::V)>;
    fn vals(&self) -> impl DoubleExact<Item = &Self::V>;
    //returns child_idx of new child
    fn insert_child(
        &mut self,
        child_addr: Self::P,
        child_key: Self::K,
        child_val: Self::V,
    ) -> usize;
    fn remove_child(
        &mut self,
        child_key: &Self::K,
        child_val: Self::V,
    ) -> Option<(Self::K, Self::V, Self::P)>;
}

/*
non unique ordered set ?
lookup by K to get a range of (K,V)
removal takes KV, insert takes KV, lookup takes K and yields a range
*/

// pub trait RevPtrINode <K,P> where K : D, P : D {
//     type Child : D;
//     ///return None if this node type doesnt store a parent ptr
//     fn parent(&self) -> Option<P>;
//     ///make noop if this node type doesnt store a parent ptr
//     fn update_parent(&mut self, p : P);
//     fn keys(&self) -> impl DoubleExact<Item=&K>;
//     fn try_route(&self, k : &K) -> Option<usize>;
//     fn child(&self, child_idx : usize) -> &Self::Child;
//     fn children(&self) -> impl DoubleExact<Item=&Self::Child>;
//     //returns child_idx of new child
//     fn insert_child(&mut self, child_addr : P, child_key : K) -> usize;
//     fn remove_child(&mut self, child_key : &K) -> Option<(K,P)>;

// }
// ///nodes which store values associated with keys, and no internal pointers within the block.
// pub trait RevPtrLNode<K,V,P : D> where K : D, V: D {
//     fn values(&self) -> impl DoubleExact<Item=&V>;
//     fn pairs(&self) -> impl DoubleExact<Item=(&K,&V)>;
//     fn keys(&self) -> impl DoubleExact<Item=&K>;
//     ///return None if this node type doesnt store a parent ptr
//     fn parent(&self) -> Option<P>;
//     ///make noop if this node type doesnt store a parent ptr
//     fn update_parent(&mut self, p : P);
// }

// ///nodes which store a single value, which may be the key , (K,V), or a distinct thing.
// pub trait RevPtrValueNode<V : D,P : D> {
//     fn value(&self) -> V;
//     ///return None if this node type doesnt store a parent ptr
//     fn parent(&self) -> Option<P>;
//     ///make noop if this node type doesnt store a parent ptr
//     fn update_parent(&mut self, p : P);
// }

/*
struct Walker<N,Ordering> {
    stack : Vec<(P,usize)>
    block : &TreeBlock<N,Ordering>
}

pub trait Walker<N,K,V,P> {
    fn current(&self) -> &N;
    fn parent(&self) -> &N;

}

ok so walker N : Splittable gates split fns like split_child and split_root
were going to have one impl of walker per ordering and for
Inode, RevptrInode, LNode, RevPtrLNode, EnumNode, UnionNode,

so our axes are basically : is it an inode, and is it a composed type, and does it store parent pointers,
Then also per ordering we have to be able to walk to prev and next
Also we're doing an impl for TaggedPtr/UnionNode for each lol.

realistically every node is a composition of inode and lnode or valuenode.
the question is if its a EnumNode (bleh) or a UnionNode
So we impl walker for
UnionNode(Inode,Lnode),
UnionNode(Inode,ValueNode),
UnionNode(RevPtrInode,RevPtrLnode),
UnionNode(RevPtrInode,RevPtrValueNode),

EnumNode(Inode,Lnode)
EnumNode(Inode,ValueNode)
EnumNode(RevPtrInode,RevPtrLnode)
EnumNode(RevPtrInode,RevPtrValueNode)

Then also the taggedptr union variants
UnionNode(INode<P=TaggedPtr>, LeafNode),
UnionNode(INode<P=TaggedPtr>, ValueNode),
UnionNode(RevPtrINode<P=TaggedPtr>, RevPtrLNode),
UnionNode(RevPtrINode<P=TaggedPtr>, RevPtrValueNode),

then we impl the split methods for each?
I think our descent is basically

//in tagged union's case.
while let Some(i) = self.try_route(&k) {
    let child = self.child(i);
    let is_leaf = child.is_leaf();
    self.push(child,i);
    if is_leaf { break }
}

//for UnionNode Inode,Lnode
for i in 0..self.tree.height {
    let Some(i) = self.try_route(&k);
    let child = self.child(i);
    self.push(child,i);
    if self.stack.len()==height { break }
}

// enum node's case
while let Inode(node) = self.current.as_inode() {
    if let Some(child_idx) = node.try_route(&k);
        let child = node.child(child_idx);
        self.push(child,i);
    else
        break
}

//then the revptr versions all do self.current = (child,i) instead of self.push((child,i))
//a 'stack' impl can be made that cheats and uses parent node ptr instead of actually maintaining a stack.
//thats half. the other 3 i think are genuinely different.
Ok 3's not bad. actually wait i forgot a case- no container, single value per node instead of leafnode.

Inode<P> + ValueNode<V> , or just raw inode.
while let Some(child_idx) = self.current.try_route(k) {
    let v = self.current().child(child_idx);
    self.stack.push((v,child_idx))
}

actually, theres a distinction - nodes where k matching => descend vs match = stop.
i guess thats try_route semantics.

so 4 types of walk, 2 types of stack, 4 types of ordering
i think height is mandatory regardless of whether its used by the stack to be able to ascend/descend
to specific places.
*/
