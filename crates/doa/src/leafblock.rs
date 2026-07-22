use std::{collections::VecDeque, marker::PhantomData, ops::{Index, IndexMut}};

use crate::index::*;

///max node cap = 256 
const MAX_NODE_CAP: usize = 16;
///block cap = full P range. u16 -> 65536 slots.
const MAX_BLOCK_CAP: usize = 65536;

///ptr into a leafblock + the leaf's len/cap. header lives in the consumer
///(btree inode's terminal PtrUnion), NOT in the block. cap slots starting at
///ptr form the leaf; len of them are occupied.
#[derive(Copy, Clone)]
pub struct SlicePtr<P: BlockIndex> {
    pub ptr: P,
    pub len: P,
}

impl<P: BlockIndex> SlicePtr<P> {
    fn new(ptr: P, len: P) -> Self { Self { ptr, len } }
    fn get_ptr(&self) -> P { self.ptr }
    fn get_len(&self) -> P { self.len }
}

///inode child pointer. internal -> another inode (in the inode block);
///terminal -> a leaf SlicePtr in the leafblock. PtrUnion<u32,u16> is 4 bytes
///either way: u32 internal, SlicePtr<u16> = 2+1+1.
pub union PtrUnion<P1, P2>
where P1: BlockIndex, P2: BlockIndex {
    pub internal: P1,
    pub terminal: SlicePtr<P2>,
}

///random leafblock: leaves scattered across the address space with None gaps
///between them so a leaf can grow by claiming adjacent gaps. no append/prepend
///optimization (the btree forest doesn't need them). block-level reorg on
///exhaustion goes through split_and_rotate (pointer-rotation trick) so no full
///readdress.
pub struct LeafBlock<K, V, P>
where K: Ord + Clone + Sized, V: Sized, P: BlockIndex {
    data: Vec<Option<(K, V)>>,
    addr_shift: u32,
    virt_offset: usize,
    rotate: u32,
    _phantom : PhantomData<P>
}

// phys = (virt + virt_offset).rotate_left(rotate) >> addr_shift
// virt = (phys << addr_shift).rotate_right(rotate) - virt_offset
// steady state: addr_shift=0, rotate=0 -> consecutive virt = consecutive phys,
// so a leaf's [ptr, ptr+cap) is a contiguous phys run (may wrap the deque -> 2 slices).

pub enum GrowErr {
    ///no adjacent gap within budget; caller may spread or split.
    NoBudget,
    ///next address not representable in P; caller must split_and_rotate.
    AddressExhaustion,
}

//implicitly in-order, root is at phys_to_virt(MIDPOINT) insert cant cross it or shift it.
///root node has some special cases to consider. 
///theyre all leaves so im only calling it a root.
impl<K,V,P> LeafBlock<K,V,P>
where   K : Ord + Clone + Sized,
        V : Sized,
        P : BlockIndex {
    /*
    //new empty random block, addr_shift=ptr::bit_width rotate=0 , v_offset=0
    fn new();

    //use index for unwrapping get.
    fn get(P) -> &Option<T>;
    
    fn get_mut(P) -> &mut Option<T>;
    
    //get an iter capable of seek forward or backward, not a cursor though.
    //only exposes T, skips None internally. 
    fn iter(&self);
    
    fn iter_mut(&mut self);  
    
    //infallible, caller guarantees node has space
    fn insert_between(&mut self, kv : (K,V), node : &mut SP, next : P); 
    
    fn phys_to_virt(P) -> P;
    
    ///dumb insert, searches the entirety of the block for the correct place to put K.
    ///assumes no slice pointers exist. shifts elements and grows freely, making no stability guarantees.
    ///panics if len is >= MAX
    fn insert_root<MAX>(&mut self, K, V);

    fn root_node(&self, &sp) -> &[Option<T>];

    fn virt_to_phys(P)->P;
    
    ///new addresses need to be recalculated using the blocks parameters. 
    fn next_virt(&self, P) -> P;
    
    fn prev_virt(&self, P) -> P; 
    
    ///inode ordering matters here, we'll assume pre-order for now so subtrees arecontinuous.
    ///in-order could work too but the maths a bit different. basically just not BF.
    fn grow_and_spread(&mut self);
    
    ///left node stays in place, right gets a new buffer of the same size. 
    fn split_and_rotate(&mut self)-> Self;

    //node management

    //calculate num of physical slots in p1..p2
    fn distance(&self, p1,p2)
    
    fn slice(&self, p, p2) -> & [Option(K,V)];
    
    fn slice_mut(&self, p, p2) -> &mut [Option(K,V)];
    
    //move a none from this to next
    fn lend_right(&mut self, this : &mut sp, next : &mut sp)
    
    //move a none from this to prev
    fn lend_left(&mut self, prev : &mut sp, this : &mut sp)
    
    //move an element from this to next
    fn shove_right(&mut self, this: &mut sp, next : &mut sp)
    
    //move an element from prev to this
    fn shove_left(&mut self, prev : &mut sp, this: &mut sp)

    //split the node at sp, move half its Some elements into the right half 
    //of its cap and return a new ptr to it.
    fn split_node(&mut self, &mut sp, next : p) -> sp

    //infallible insert, panics if node doesnt have space. 
    fn node_insert(&mut self, &mut sp, next : p)
    */
}


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
//     fn index_mut(&mut self, rel: P::Half) -> &mut Self::Output {
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