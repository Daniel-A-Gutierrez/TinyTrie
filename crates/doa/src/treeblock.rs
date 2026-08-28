use crate::{Fixup, Ordering, RootPos, TreeOrdering, blocks::BlockTrait, index::*, store::{DequeStore, NoneSlide, Store, VecStore}, translator::{AddressTranslator, Translator}, walker::{Node, TreeWalker}};
use crate::blocks::*;
use crate::walker::*;
use std::marker::PhantomData;

pub trait TreeBlock<'block> : BlockTrait<'block>
where Self::T : Node + Default,
Self::O : TreeOrdering,
{
    type W<'walker> : TreeWalker<'block,'walker,Self> where 'block : 'walker, Self:'walker;
    type WM<'walker> : TreeWalkerMut<'block,'walker,Self> where 'block : 'walker, Self:'walker;
    type K;
    type V;
    ///phys slot of the root node. FixedRoot modes derive it from the translator; movable-root
    ///modes read it from `BlockData` (whose impl then bounds `BD: HasRoot`).
    fn root_position(&self) -> usize;
    //creates a walker, uses it to lookup node, returns position
    fn walker(&self, key : Self::K) -> Self::W<'_>;
    fn walker_mut(&mut self, key : Self::K) -> Self::WM<'_>;
    fn insert_child();
    fn remove_child();
}

pub trait SplitTreeBlock<'block, O: TreeOrdering> : TreeBlock<'block>
where Self::T : Node + Default,
    Self::O : TreeOrdering
{
    //or something
    fn split_root(self) -> Self;
}


/*
struct BTreeBlock<K,V,P> {
    block : UTreeBlock<Node<K,V,P>, PreOrder>
}

type UTreeBlock<T : Node<K,V,P>, O : Ordering> = TreeBlock<T,O,Uniform>
*/