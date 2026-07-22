///a structure for storing leaves pointed at by another structure
///a pluripotent block of inodes may need to store PTR::MAX()*FANOUT leaves
///instead of being forced into using a wider ptr type, the leafblock provides a solution
///by making individual items unaddressable, working over slices instead. 
///Its effectively a vec of inline vecs.
use crate::index::*;
use crate::InsertDelta;
use std::marker::PhantomData;
use std::{cmp::Ordering::{Equal, Greater, Less}, collections::VecDeque, ops::Range};

trait Mode<T : Sized + Copy>{type Rep : Sized + Copy;}
struct Sparse{}
struct Dense{}

impl<T : Sized + Copy> Mode<T> for Sparse{type Rep = Option<T>;}
impl<T : Sized + Copy> Mode<T> for Dense{type Rep = T;}

#[derive(Copy,Clone)]
///f must be large enough to store the MIN/MAX values, P is the pointer type into the block.
///used when leafnode headers are stored inline in the array, taking up 1 slot and managing the following CAP slots.
struct Header<F : UnsignedNum, P : BlockIndex> {
    len : F , 
    cap : F,
    next : P,
    prev : P
}

///The data type for an array of leafnodes where the leafnode header is stored inline within the array. 
union UData<T : Sized + Copy, F: UnsignedNum, P : BlockIndex, M : Mode<T>> 
where M::Rep : Sized + Copy, T : Sized + Copy {
    header: Header<F,P>,
    data : M::Rep,
}

///The data type for an array of leafnodes where the leafnode header is stored inline within the array as an enum. 
enum EData<T : Sized + Copy, F: UnsignedNum, P : BlockIndex, M : Mode<T>> 
where M::Rep : Sized + Copy, T : Sized + Copy {
    Header( Header<F,P> ), 
    Data(M::Rep),
}

///A block specialized for storing leaf node headers inline alongsize their keys/values.
///T : stored type (LeafNode)
///P : Pointer type used to point into data
///MIN,MAX : The minimum and maximum size of a block of leafnodes
struct LeafBlock<T, F, P, M, const MAX : usize, const MIN :usize,> 
where T : Sized + Copy , F : UnsignedNum, P : BlockIndex, M : Mode<T> {
    data : VecDeque<UData<T,F,P,M>>,
    phantom : PhantomData<(P, M)>
}

//need
struct PartSplitErr{}

///a view into a leafblock, pointing at a header P
struct LeafNode<'a, T, P, F, M, const MIN : usize, const MAX : usize> 
where T: Sized + Copy, P : BlockIndex, F : UnsignedNum, M : Mode<T>{
    owner : &'a LeafBlock<T,F,P,M,MAX,MIN>,
    header : P,
    _phantom : PhantomData<M>
}
///a mutable view into a leafblock pointing at a header P
struct LeafNodeMut<'a, T, P, F, M, const MIN : usize, const MAX : usize> 
where T: Sized + Copy, P : BlockIndex, F : UnsignedNum, M : Mode<T>{
    owner : &'a mut LeafBlock<T,F,P,M,MAX,MIN>,
    header : P,
    _phantom : PhantomData<M>
}

impl<P, T, F, M, const MIN : usize, const MAX : usize> LeafBlock<T,F,P,M,MIN,MAX>
where
    P : BlockIndex,
    T : Sized + Copy,
    F : UnsignedNum,
    M : Mode<T>,
{
    fn get(&self, ptr : P) -> LeafNode<T,P,F,M,MIN,MAX> {todo!()}
    fn get_mut(&mut self, ptr : P) ->LeafNodeMut<T,P,F,M,MIN,MAX>{todo!()}
    fn get_node(&self, ptr : P, idx : usize) -> & Option<T> {todo!()}
    fn get_node_mut(&mut self, ptr :P, idx : usize) ->&mut Option<T> {todo!()}
    ///split a block in 2, with the right half getting the elements from idx..end
    ///panics if idx is out of bounds, err if there isnt enough space around the partition to split
    ///right partition 
    fn split(&mut self, ptr : P, idx : usize) -> Result<InsertDelta<T>,PartSplitErr>{todo!();}
    ///look around for nearby space between self and prev/next, grow up to amount. 
    fn grow(&mut self, amount : usize, budget : usize){todo!()}
    fn spread(&mut self) {todo!()}
    fn insert(&mut self, position : P, size : usize, budget : usize){ todo!() }
    fn append(&mut self) {todo!()}
    fn remove(&mut self) {todo!()}
}