//pub mod block;
//pub mod block_cursor;
pub mod index;
mod inline_leafblock;
mod leafblock;
mod blocks;
pub mod metadata;
mod treeblock;
//pub mod node;
pub mod store;
pub mod translator;
pub mod walker;
use crate::leafblock::{PtrUnion, SlicePtr};
use crate::translator::{AddressTranslator, Translator};
use blocks::*;
use index::*;
//use node::*;
use std::{cmp::Ordering::{Equal, Greater, Less},
          collections::VecDeque,
          marker::PhantomData,
          ops::Range};

pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
///non-tree ordering: a sorted sequence block (no tree root; `ROOT_POS` unused).
pub struct Sorted;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPos { Beginning, Middle, End }

pub trait Ordering: 'static {
    ///where the tree root lives in a fresh block.
    const ROOT_POS: RootPos;
}

///tree ordering: defines a root position + an in-order traversal the block lays out
///contiguously. `TreeBlock`/`TreeWalker` gate `O` on this; non-tree blocks may use a plain
///`Ordering` (e.g. `Sorted`).
pub trait TreeOrdering: Ordering {}

///easiest to split, iteration OK
impl Ordering for InOrder   { const ROOT_POS: RootPos = RootPos::Middle; }
impl Ordering for PreOrder  { const ROOT_POS: RootPos = RootPos::Beginning; }
impl Ordering for PostOrder { const ROOT_POS: RootPos = RootPos::End; }
impl TreeOrdering for InOrder {}
impl TreeOrdering for PreOrder {}
impl TreeOrdering for PostOrder {}
///non-tree: ROOT_POS is a placeholder, never read by a sorted-array block.
impl Ordering for Sorted   { const ROOT_POS: RootPos = RootPos::Beginning; }

enum RelTo<T> {
    Before(T),
    After(T),
}
pub(crate) type BPtr = i32;
pub(crate) type IPtr = u32;
pub(crate) type LPtr = u16;

pub use metadata::Fixup;



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
