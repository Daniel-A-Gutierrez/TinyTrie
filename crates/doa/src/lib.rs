mod abstract_tree;
mod alloc_strat;
mod block;
mod index;
mod inline_leafblock;
mod leafblock;
mod store;
mod translator;
mod tree_traits;
use crate::leafblock::{PtrUnion, SlicePtr};
use block::*;
use index::*;
use crate::translator::{Translator, AddressTranslator};
use std::{cmp::Ordering::{Equal, Greater, Less},
          collections::VecDeque,
          marker::PhantomData,
          ops::Range};
use tree_traits::*;

pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
pub trait Ordering: 'static {}

///easiest to split, iteration OK
impl Ordering for InOrder {}
impl Ordering for PostOrder {}
impl Ordering for PreOrder {}

enum RelTo<T> {
    Before(T),
    After(T),
}
pub(crate) type BPtr = i32;
pub(crate) type IPtr = u32;
pub(crate) type LPtr = u16;

//fractal forest
struct FractalForest<K: Ord + Sized + Clone, V: Sized> {

    ///root is at trees[0]
    root:   BTree<K, BPtr>, //map key to a terminal block
    ltrees: Vec<BTree<K, V>>,
    len:    usize,
}




struct BTree<K: Sized + Ord + Clone, V: Sized> {

    // inodes : block::Block<INode<K, IPtr, LPtr>,IPtr,PreOrder,Pluripotent>, //require preorder and fixed root, and pluripotent
    leaves: leafblock::LeafBlock<K, V, LPtr>, //leafblock is random so it can guarantee capacity so long as inodes max size is 4096 (for u16, fanout 16)
    height: u32,
    next:   u32,
    prev:   u32,
}



impl<K, V> BTree<K, V>
where
    K: Sized + Ord + Clone,
    V: Sized,
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
