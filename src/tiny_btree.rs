use std::mem::MaybeUninit;
use std::{cmp::Ordering, marker::PhantomData};
use std::num::{NonZero,ZeroablePrimitive};

pub trait TrieIndex: Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug + 'static + ZeroablePrimitive{
    /// Convert to `usize` for indexing.
    fn as_usize(self) -> usize;
    /// Maximum representable value (e.g. `u16::MAX` for u16).
    fn max_value() -> usize;
    /// Maximum value used as sentinel for empty slots in `children[]`.
    /// With stacking, encoded addresses use 0 as a valid address (root = phys 0, vnode 0),
    /// so `PTR::MAX` is the sentinel instead of 0.
    fn from_usize(n: usize) -> Self;
}


trait TreeKey : PartialOrd + PartialEq + Eq + Ord + Sized {
    //find index where this element would be inserted in a sorted array, were it inserted.
    #[inline] fn cmp_many<const N : usize>(&self, other : &[Self;N]) -> usize {todo!();}
}

struct KeyNode<K, PTR : TrieIndex, const N : usize> where K : TreeKey, [();N+1]: {
    len : u8,
    keys : [MaybeUninit<K>;N],
    ptrs : [Option<NonZero<PTR>>;N+1],
}

struct ValueNode<V, const N : usize> where V : Sized, [();N]: {
    len : u8,
    vals : [MaybeUninit<V>;N]
}

///keys are stored internally. the value corresponding to each key is 
///at an identical arena+node index to its key, in values.
struct DualTree<K,V,PTR,const N : usize> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    arena : Vec<KeyNode<K,PTR,N>>,
    values : Vec<ValueNode<V,N>>,
    len : usize
}

struct Cursor<'a,K,V,PTR,const N : usize> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    tree : &'a DualTree<K,V,PTR,N>,
    stack : Vec<usize>,
    position : usize,
    phantom : PhantomData<V>
}

struct CursorMut<'a,K,V,PTR,const N : usize> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    tree : &'a mut DualTree<K,V,PTR,N>,
    stack : Vec<usize>,
    position : usize,
    phantom : PhantomData<V>
}

impl<K, PTR : TrieIndex, const N : usize> KeyNode<K,PTR,N> 
    where K : TreeKey, [();N+1]: {
        //takes the keys and ptrs in the range specified, 0s the rest.
        fn from_parent(from : u8, to : u8, parent : &Self) -> Self{todo!();}
        fn new() -> Self{todo!();}
        #[inline] fn get(&self, i : u8) -> &K {todo!();}
        #[inline] fn get_ptr(&self, i : u8) -> Option<PTR> {todo!();}
        #[inline] unsafe fn get_unchecked(&self, i : u8) -> &K {todo!();}
        #[inline] unsafe fn get_ptr_unchecked(&self, i : u8) -> Option<PTR> {todo!();}
        #[inline] fn find_position(&self, k : &K) -> u8 {todo!();}
        ///removing an element would overflow the node
        #[inline] fn would_split(&self, k : &K) -> bool {self.len == N as u8}
        ///removing an element would drop len below the minimum
        #[inline] fn would_merge(&self, k : &K) -> bool {self.len == N as u8 / 2}
        ///returns position leaf was inserted at. 
        ///caller guarantees would_split has returned false before calling this. 
        #[inline] fn insert_leaf(&mut self, k: K) -> u8 {todo!();}
        ///this node becomes the parent, initializes 2 new children.
        ///assumes the children can be placed at l_addr and r_addr.
        ///also inserts k
        #[inline] fn remove(&mut self, pos: u8) -> K {todo!();}
        #[inline] fn truncate(&mut self, newlen : u8) {self.len = newlen}
}

impl<V, const N : usize> ValueNode<V,N> 
    where V : Sized, [();N]: {
    fn insert(&mut self,  pos : u8, val : V) -> u8 {todo!();}
    fn remove(&mut self, pos: u8) -> V {todo!();}
    fn from_slice(src:&[V]) -> Self {todo!();}
    fn truncate(&mut self, newlen: u8) { self.len = newlen }
}

impl<K,V,PTR,const N : usize> DualTree<K,V,PTR,N> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");
    pub fn get(&self, key : &K) -> &V {todo!();}
    pub fn get_mut(&self, key : &K) -> &V {todo!();}
    pub fn get_cursor(&self) -> Cursor<K,V,PTR,N> {todo!();}
    pub fn get_cursor_mut(&self) -> Cursor<K,V,PTR,N> {todo!();}
    pub fn insert(&self, key : K, value : V) -> Result<(), (K,V)> {todo!();}
    fn get_idx(&self, key : &K) -> Option<(usize,u8)> {todo!();}
}

impl <'a,K,V,PTR,const N : usize>  Cursor<'a,K,V,PTR,N> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    pub fn current(&self) -> (&K,&V) {todo!();}
    pub fn advance_next(&self) {todo!();}
    pub fn advance_prev(&self) {todo!();}
    pub fn next(&self) -> Option<&V> {todo!();}
    pub fn prev(&self) -> Option<&V> {todo!();}
}

impl <'a,K,V,PTR,const N : usize>  CursorMut<'a,K,V,PTR,N> 
where  K : TreeKey, 
    [();N]:, 
    [();N+1]:,
    V : Sized, 
    PTR : TrieIndex {
    pub fn current(&mut self) -> (&K,&V) {todo!();}
    pub fn advance_next(&mut self) {todo!();}
    pub fn advance_prev(&mut self) {todo!();}
    pub fn next(&mut self) -> Option<&V> {todo!();}
    pub fn prev(&mut self) -> Option<&V> {todo!();}
}