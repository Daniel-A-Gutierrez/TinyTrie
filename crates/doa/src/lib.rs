mod abstract_tree;
mod index;
mod block;
mod inline_leafblock;
mod leafblock;
mod tree_traits;
use std::{cmp::Ordering::{Equal, Greater, Less}, collections::VecDeque, marker::PhantomData, ops::Range};
use block::*;
use index::*;
use crate::leafblock::{PtrUnion, SlicePtr};

type BPtr = i32;
type IPtr = u32;
type LPtr = u16;
//fractal forest
struct FractalForest<K : Ord + Sized + Clone, V : Sized> {
    ///root is at trees[0]
    root : BTree<K,BPtr>, //map key to a terminal block
    ltrees : Vec<BTree<K,V>>,
    len : usize
}
///in a b+tree theres 1 more key per value for inodes 
///generic const exprs doesnt support N+1 as an array size across crate boundaries. 
struct INode<K : Sized + Ord, I : BlockIndex, L : BlockIndex> {
    keys : [K;16],
    leaves : [PtrUnion<I,L>; 17]
}

struct BTree<K : Sized + Ord + Clone, V : Sized> {
    inodes : block::Block<INode<K, IPtr, LPtr>,IPtr,PreOrder,Pluripotent>, //require preorder and fixed root, and pluripotent
    leaves : leafblock::LeafBlock<K,V,LPtr>, //leafblock is random so it can guarantee capacity so long as inodes max size is 4096 (for u16, fanout 16)
    height : u32,
    next : u32,
    prev : u32,
}

impl<K,V> BTree<K,V> 
where 
    K : Sized + Ord + Clone,
    V : Sized 
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

// -----------------------------------------------------------------------
// InsertDelta — remap info produced by insert/remove for pointer fixup.
// -----------------------------------------------------------------------
#[derive(Debug)]
pub enum InsertDelta<T> {
    /// No work necessary for caller. 
    Free { new_virt: isize },

    /// Elements were shifted to create room (insert) or re-align vacancies
    /// (remove). 
    /// `new_virt` = the new element's address (insert) or the shifted
    /// region's anchor (removal). 
    /// `amount` = positions each element moved;
    /// `minus` = per-element address-delta sign (`+1`/`-1`); 
    /// `addr_delta` = `minus << addr_shift` (the remap callers apply).
    Move { new_virt: isize, amount: usize, minus: isize, addr_delta: isize },

    ///placeholder
    BlockSplit { _phantom: PhantomData<T> },
}
