```rust
//! leafblock variant with per-leaf headers stored inline among the slots — compiled, unwired, pre-refactor sketch (all bodies todo!()).
///L0014
///a structure for storing leaves pointed at by another structure
///a pluripotent block of inodes may need to store PTR::MAX()*FANOUT leaves
///instead of being forced into using a wider ptr type, the leafblock provides a solution
///by making individual items unaddressable, working over slices instead.
///Its effectively a vec of inline vecs.
struct Sparse {}
struct Dense {}
#[derive(Copy, Clone)]
///f must be large enough to store the MIN/MAX values, P is the pointer type into the block.
///used when leafnode headers are stored inline in the array, taking up 1 slot and managing the following CAP slots.
struct Header<F: UnsignedNum, P: BlockIndex> {
    len:  F,
    cap:  F,
    next: P,
    prev: P,
}
///L0029
///The data type for an array of leafnodes where the leafnode header is stored inline within the array.
union UData<T: Sized + Copy, F: UnsignedNum, P: BlockIndex, M: Mode<T>>
where
    M::Rep: Sized + Copy,
    T: Sized + Copy,
{
    header: Header<F, P>,
    data:   M::Rep,
}
///L0039
///The data type for an array of leafnodes where the leafnode header is stored inline within the array as an enum.
enum EData<T: Sized + Copy, F: UnsignedNum, P: BlockIndex, M: Mode<T>>
where
    M::Rep: Sized + Copy,
    T: Sized + Copy,
{
    Header(Header<F, P>),
    Data(M::Rep),
}
///L0052
///A block specialized for storing leaf node headers inline alongsize their keys/values.
///T : stored type (LeafNode)
///P : Pointer type used to point into data
///MIN,MAX : The minimum and maximum size of a block of leafnodes
struct LeafBlock<T, F, P, M, const MAX: usize, const MIN: usize>
where
    T: Sized + Copy,
    F: UnsignedNum,
    P: BlockIndex,
    M: Mode<T>,
{
    data:    VecDeque<UData<T, F, P, M>>,
    phantom: PhantomData<(P, M)>,
}
//need
///L0064
struct PartSplitErr {}
///a view into a leafblock, pointing at a header P
struct LeafNode<'a, T, P, F, M, const MIN: usize, const MAX: usize>
where
    T: Sized + Copy,
    P: BlockIndex,
    F: UnsignedNum,
    M: Mode<T>,
{
    owner:    &'a LeafBlock<T, F, P, M, MAX, MIN>,
    header:   P,
    _phantom: PhantomData<M>,
}
///L0080
///a mutable view into a leafblock pointing at a header P
struct LeafNodeMut<'a, T, P, F, M, const MIN: usize, const MAX: usize>
where
    T: Sized + Copy,
    P: BlockIndex,
    F: UnsignedNum,
    M: Mode<T>,
{
    owner:    &'a mut LeafBlock<T, F, P, M, MAX, MIN>,
    header:   P,
    _phantom: PhantomData<M>,
}
///L0092
trait Mode<T: Sized + Copy> {
    type Rep: Sized + Copy;
}
///L0096
impl<T: Sized + Copy> Mode<T> for Sparse {}
///L0100
impl<T: Sized + Copy> Mode<T> for Dense {}
///L0104
impl<P, T, F, M, const MIN: usize, const MAX: usize> LeafBlock<T, F, P, M, MIN, MAX>
where
    P: BlockIndex,
    T: Sized + Copy,
    F: UnsignedNum,
    M: Mode<T> {}
```
