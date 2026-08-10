use std::marker::PhantomData;

use crate::alloc_strat::AllocStrat;
use crate::block_cursor::{BlockCursor};
use crate::block::{BlockMutTrait, BlockTrait, OpenSlot, RawBlock};
use crate::node::Node;
use crate::store::{NoneSlide, Store};
use crate::translator::Translator;
use crate::{Ordering, index::*};
pub struct TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T>,
    O: Ordering,
    Meta: Sized + 'static,
{
    meta:  Meta,
    block: RawBlock<'a, T, P, A, S>,
    root : P,
    _o:    PhantomData<O>,
}

pub(crate) trait TreeBlockMut<'a>: BlockMutTrait<'a> + 'a
where Self::T: Node
{
    type Meta;
    type K;
    type V;
    type O;
    fn meta(&self) -> &Self::Meta;
    fn set_meta(&mut self, m: Self::Meta);
    fn root(&self) -> Self::P;
    fn set_root(&mut self, p : Self::P);
}

impl<'a, T, P, A, S, O, Meta> BlockTrait<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static,
{
    type T = T;
    type P = P;
    type S = S;
    type Cursor<'b> = BlockCursor<'a, 'b, Self, &'b Self>
    where 'a: 'b;

    fn store<'b>(&'b self) -> &'b Self::S
    where 'a: 'b {
        self.block.store()
    }

    fn translator<'b>(&'b self) -> &'b Translator<Self::P> {
        self.block.translator()
    }

    fn cursor<'b>(&'b self) -> Self::Cursor<'b>
    where 'a: 'b {
        BlockCursor::new(self)
    }

    ///delegate so REVERSED strategies still iterate high→low.
    fn iter<'b>(&'b self) -> impl ExactSizeIterator<Item = &'b Self::T> + 'b
    where 'a: 'b {
        self.block.iter()
    }
}

impl<'a, T, P, A, S, O, Meta> BlockMutTrait<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static + Default + Clone,
    RawBlock<'a, T, P, A, S>: BlockMutTrait<'a, A = A> + BlockTrait<'a, T = T, P = P, S = S>,
{
    type A = A;
    type CursorMut<'b> = BlockCursor<'a, 'b, Self, &'b mut Self>
    where 'a: 'b;
    fn new() -> Self {
        Self { meta: Meta::default(), block: RawBlock::new(), _o: PhantomData, root : A::INIT_ROOT }
    }

    fn store_mut(&mut self) -> &mut Self::S {
        self.block.store_mut()
    }
    fn translator_mut(&mut self) -> &mut Translator<Self::P> {
        self.block.translator_mut()
    }

    fn cursor_mut<'b>(&'b mut self) -> Self::CursorMut<'b>
    where 'a: 'b {
        BlockCursor::new(self)
    }

    fn insert(&mut self, v: Self::T, slot: OpenSlot) -> usize {
        self.block.insert(v, slot)
    }
    ///that this is a naieve version for the trait impl, it doesnt produce a valid
    ///treeblock on its own. it presumes the root of the right half is at 'at'
    fn split(&mut self, at : usize) -> Self {
        let r = self.block.split_block(at);
        Self { 
            _o : PhantomData,
            meta : self.meta.clone(),
            block : r,
            root : P::ZERO //inorder : 0, preorder :0, postorder : len TODO
        }
    }

    ///root needs setting after this for the right half. 
    fn split_and_rotate(&mut self, at : usize) -> Self {
        let r = self.block.split_and_rotate(at);
        Self { 
            _o : PhantomData,
            meta : self.meta.clone(),
            block : r,
            root : P::ONE //inorder : 0, preorder :0, postorder : len TODO : fix root after
        }    
    }

    fn try_insert_back(&mut self, v: Self::T) -> Result<usize, Self::T> {
        self.block.try_insert_back(v)
    }
    fn try_insert_front(&mut self, v: Self::T) -> Result<usize, Self::T> {
        self.block.try_insert_front(v)
    }
}

impl<'a, T, P, A, S, O, Meta> TreeBlockMut<'a> for TreeBlock<'a, T, P, A, S, O, Meta>
where
    T: Sized + Node + 'a,
    P: BlockIndex,
    A: AllocStrat<P>,
    S: Store<'a, T> + 'a,
    O: Ordering,
    Meta: Sized + 'static + Default + Clone,
    RawBlock<'a, T, P, A, S>: BlockMutTrait<'a, A = A> + BlockTrait<'a, T = T, P = P, S = S>,
{
    type Meta = Meta;
    type K = T::K;
    type V = T::V;
    type O = O;
    fn meta(&self) -> &Meta {
        return &self.meta;
    }
    fn set_meta(&mut self, m: Meta) {
        self.meta = m
    }
    fn root(&self) -> Self::P { self.root }
    fn set_root(&mut self, p : Self::P) {
        self.root = p;
    }
}
