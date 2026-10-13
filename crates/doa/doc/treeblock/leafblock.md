```rust
//! random leafblock: fixed-cap leaf slices scattered across one block's address space — compiled, unwired, predates the BlockOps refactor (half the surface is commented out).
///L0009
///max node cap = 256
const MAX_NODE_CAP: usize = 16;
///L0012
///block cap = full P range. u16 -> 65536 slots.
const MAX_BLOCK_CAP: usize = 65536;
///L0018
///ptr into a leafblock + the leaf's len/cap. header lives in the consumer
///(btree inode's terminal PtrUnion), NOT in the block. cap slots starting at
///ptr form the leaf; len of them are occupied.
#[derive(Copy, Clone)]
pub struct SlicePtr<P: BlockIndex> {
    pub ptr: P,
    pub len: P,
}
///L0027
///inode child pointer. internal -> another inode (in the inode block);
///terminal -> a leaf SlicePtr in the leafblock. PtrUnion<u32,u16> is 4 bytes
///either way: u32 internal, SlicePtr<u16> = 2+1+1.
#[derive(Copy, Clone)]
pub union PtrUnion<P1, P2>
where
    P1: BlockIndex,
    P2: BlockIndex,
{
    pub internal: P1,
    pub terminal: SlicePtr<P2>,
}
///L0041
///random leafblock: leaves scattered across the address space with None gaps
///between them so a leaf can grow by claiming adjacent gaps. no append/prepend
///optimization (the btree forest doesn't need them). block-level reorg on
///exhaustion goes through split_and_rotate (pointer-rotation trick) so no full
///readdress.
pub struct LeafBlock<K, V, P>
where
    K: Ord + Clone + Sized,
    V: Sized,
    P: BlockIndex,
{
    data:        Vec<Option<(K, V)>>,
    addr_shift:  u32,
    virt_offset: usize,
    rotate:      u32,
    _phantom:    PhantomData<P>,
}
// phys = (virt + virt_offset).rotate_left(rotate) >> addr_shift
// virt = (phys << addr_shift).rotate_right(rotate) - virt_offset
// steady state: addr_shift=0, rotate=0 -> consecutive virt = consecutive phys,
// so a leaf's [ptr, ptr+cap) is a contiguous phys run (may wrap the deque -> 2 slices).
///L0058
pub enum GrowErr {
    ///no adjacent gap within budget; caller may spread or split.
    NoBudget,
    ///next address not representable in P; caller must split_and_rotate.
    AddressExhaustion,
}
///L0066
impl<P: BlockIndex> SlicePtr<P> {}
//implicitly in-order, root is at phys_to_virt(MIDPOINT) insert cant cross it or shift it.
///L0083
///root node has some special cases to consider.
///theyre all leaves so im only calling it a root.
impl<K, V, P> LeafBlock<K, V, P>
where
    K: Ord + Clone + Sized,
    V: Sized,
    P: BlockIndex {}
//borrowed window over a leaf's [ptr, ptr+cap). the run may wrap the VecDeque,
//so it's two slices: logical index 0..a.len() in `a`, the rest in `b`.
//cap = a.len() + b.len(). read-only; indexes directly, no per-index translation.
// pub struct LeafNode<'a, K, V, P: BlockIndex>
// where K: Ord + Clone + Sized, V: Sized {
//     data: &'a [Option<(K, V)>],
//     sp : &'a SlicePtr<P>,
//     _p: PhantomData<P>,
// }
// //need to have the llm do a pass to fix the other node stuff.
// pub struct LeafNodeMut<'a, K, V, P: BlockIndex>
// where K: Ord + Clone + Sized, V: Sized {
//     data: &'a mut [Option<(K, V)>],
//     sp :  &'a mut SlicePtr<P>,
//     _p: PhantomData<P>,
// }
// fn split_idx(a_len: usize, rel: usize) -> (bool, usize) {
//     if rel < a_len { (true, rel) } else { (false, rel - a_len) }
// }
// impl<'a, K, V, P> LeafNode<'a, K, V, P>
// where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
//     ///slot at logical position rel (may be None — leaf is sparse).
//     pub fn get(&self, rel: P::Half) -> Option<&'a (K, V)> {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         let slot = if in_a { &self.a[local] } else { &self.b[local] };
//         slot.as_ref()
//     }
//     pub fn iter(&self) -> NodeIter<'a, K, V> { NodeIter { a: self.a, b: self.b, idx: 0 } }
//     /*
//     needs more functionality to support capacity management by parent
//     fn insert( item : (K,V) )
//     fn get_capacity( self, next )
//     */
// }
// impl<'a, K, V, P> Index<P::Half> for LeafNode<'a, K, V, P>
// where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
//     type Output = Option<(K, V)>;
//     fn index(&self, rel: P::Half) -> &Self::Output {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         if in_a { &self.a[local] } else { &self.b[local] }
//     }
// }
// impl<'a, K, V, P> LeafNodeMut<'a, K, V, P>
// where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
//     pub fn cap(&self) -> usize { self.a.len() + self.b.len() }
//     pub fn get(&self, rel: P::Half) -> Option<&(K, V)> {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         let slot = if in_a { &self.a[local] } else { &self.b[local] };
//         slot.as_ref()
//     }
//     pub fn get_mut(&mut self, rel: P::Half) -> Option<&mut (K, V)> {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         let slot = if in_a { &mut self.a[local] } else { &mut self.b[local] };
//         slot.as_mut()
//     }
//     /*
//     needs more functionality to support capacity management by parent
//     pub fn insert(&mut self, k: K, v: V) { todo!() }
//     pub fn remove(&mut self, rel: P::Half) -> Option<(K, V)> { todo!() }
//     pub fn
//     */
//     ///remove the element at rel; slot becomes None.
//     pub fn iter(&self) -> NodeIter<'_, K, V> { NodeIter { a: self.a, b: self.b, idx: 0 } }
//     pub fn iter_mut(&mut self) -> NodeIterMut<'_, K, V> { NodeIterMut { a: self.a, b: self.b, idx: 0 } }
// }
// impl<'a, K, V, P> Index<P::Half> for LeafNodeMut<'a, K, V, P>
// where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
//     type Output = Option<(K, V)>;
//     fn index(&self, rel: P::Half) -> &Self::Output {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         if in_a { &self.a[local] } else { &self.b[local] }
//     }
// }
// impl<'a, K, V, P> IndexMut<P::Half> for LeafNodeMut<'a, K, V, P>
// where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
//     fn index_mut(&self, rel: P::Half) -> &mut Self::Output {
//         let (in_a, local) = split_idx(self.a.len(), rel.as_usize());
//         if in_a { &mut self.a[local] } else { &mut self.b[local] }
//     }
// }
// ///ordered walk over a leaf's sparse slots, skipping Nones. physical order =
// ///sorted order, so a then b yields ascending keys.
// pub struct NodeIter<'a, K, V>
// where K: Ord + Clone + Sized, V: Sized {
//     a: &'a [Option<(K, V)>],
//     b: &'a [Option<(K, V)>],
//     idx: usize,
// }
// impl<'a, K, V> Iterator for NodeIter<'a, K, V>
// where K: Ord + Clone + Sized, V: Sized {
//     type Item = (&'a K, &'a V);
//     fn next(&mut self) -> Option<Self::Item> {
//         let total = self.a.len() + self.b.len();
//         while self.idx < total {
//             let (in_a, local) = split_idx(self.a.len(), self.idx);
//             self.idx += 1;
//             let slot = if in_a { &self.a[local] } else { &self.b[local] };
//             if let Some((k, v)) = slot { return Some((k, v)); }
//         }
//         None
//     }
// }
// ///lending iter: returned &mut V borrows &mut self, not 'a.
// pub struct NodeIterMut<'a, K, V>
// where K: Ord + Clone + Sized, V: Sized {
//     a: &'a mut [Option<(K, V)>],
//     b: &'a mut [Option<(K, V)>],
//     idx: usize,
// }
// impl<'a, K, V> NodeIterMut<'a, K, V>
// where K: Ord + Clone + Sized, V: Sized {
//     pub fn next(&mut self) -> Option<(&K, &mut V)> { todo!() }
// }
```
